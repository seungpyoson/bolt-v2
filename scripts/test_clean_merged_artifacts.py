#!/usr/bin/env python3
"""Test suite for scripts/clean_merged_artifacts.py.

Runs as a plain script (`python3 scripts/test_clean_merged_artifacts.py`).
Every test builds a throwaway git repo in a tmpdir with isolated git config so
no inherited ~/.gitconfig leaks in. Tests assert the safety-critical paths
identified across three adversarial review rounds (issue #802).

Coverage map (by review finding):
  P0 lane inversion          : test_lane_w_owns_worktree_bound_branch
  P0 gh SHA-bound authority  : test_headRefOid_mismatch_kept,
                               test_baseRefName_mismatch_kept,
                               test_cross_repo_pr_kept
  P0 ignored content         : test_lane_w_refuses_ignored_content,
                               test_discard_ignored_override
  P0 hidden index bits       : test_assume_unchanged_refused,
                               test_skip_worktree_refused
  P0 worktree-bound CAS      : test_lane_r_skips_worktree_bound
  P1 gh error/timeout        : test_gh_error_kept, test_gh_timeout_kept
  P1 dry-run mutates nothing : test_dry_run_mutates_nothing
  P1 -d not -D               : test_unmerged_kept_by_lane_h
  P1 trunk/current never     : test_trunk_and_current_kept
  P2 audit log location      : test_audit_log_under_git_common_dir
  P2 kill switch             : test_kill_switch
  P2 branch-name reuse       : test_branch_reuse_after_merge
  P2 backup ref shape        : test_backup_ref_is_sha_addressed
  #1050 merge-wave sync      : test_sync_main_dry_run_changes_nothing,
                               test_sync_main_dry_run_does_not_claim_stale_tracking_ref_is_current,
                               test_sync_main_dry_run_reports_cleanup_after_preview_sync,
                               test_sync_main_apply_fast_forwards_local_main,
                               test_sync_main_apply_updates_unchecked_out_main_ref,
                               test_sync_main_apply_refuses_non_fast_forward,
                               test_sync_main_apply_refusal_stops_cleanup_lanes,
                               test_sync_main_apply_refuses_dirty_checked_out_main,
                               test_sync_main_dry_run_refuses_dirty_checked_out_main_before_cleanup_preview,
                               test_sync_main_dry_run_refuses_when_preview_temp_ref_delete_fails,
                               test_sync_main_dry_run_refuses_when_preview_temp_ref_cannot_be_deleted,
                               test_remote_branch_gone_but_merged_to_main_is_cleaned_after_sync
  #1050 post-review safety   : test_lane_w_refuses_branch_delete_when_tip_drifts_to_unmerged_commit,
                               test_fetch_failure_reason_redacts_url_credentials,
                               test_report_error_redacts_common_secret_forms,
                               test_live_cache_entry_with_malformed_prs_fails_closed_without_refetch,
                               test_lane_summary_prints_full_refusal_reason
"""

from __future__ import annotations

import ast
import contextlib
import datetime as dt
import hashlib
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from typing import Any, Callable
from unittest import mock

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT))

import clean_merged_artifacts as cm  # type: ignore  # noqa: E402

GIT_ENV = {
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_CONFIG_SYSTEM": "/dev/null",
    "GIT_AUTHOR_NAME": "t",
    "GIT_AUTHOR_EMAIL": "t@t",
    "GIT_COMMITTER_NAME": "t",
    "GIT_COMMITTER_EMAIL": "t@t",
    "HOME": "/tmp",  # ensure no user gitconfig leaks
}


def _run(args: list[str], *, cwd: pathlib.Path, env: dict[str, str] | None = None,
         check: bool = True, timeout: float = 30) -> subprocess.CompletedProcess[str]:
    full_env = os.environ.copy()
    full_env.update(GIT_ENV)
    if env:
        full_env.update(env)
    return subprocess.run(
        args, cwd=cwd, check=check, capture_output=True, text=True,
        env=full_env, timeout=timeout,
    )


def git(cwd: pathlib.Path, *args: str) -> str:
    return _run(["git", *args], cwd=cwd).stdout


def git_common_dir_compat(work: pathlib.Path) -> pathlib.Path:
    if not work.is_dir():
        raise FileNotFoundError(f"Work directory {work} does not exist")
    common_dir = pathlib.Path(
        _run(["git", "rev-parse", "--git-common-dir"], cwd=work).stdout.strip()
    )
    if not common_dir.is_absolute():
        common_dir = work / common_dir
    return common_dir.resolve()


def file_sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def make_repo(tmp: pathlib.Path) -> pathlib.Path:
    """Bare remote + working clone. Returns the working repo path."""
    remote = tmp / "remote.git"
    _run(["git", "init", "--bare", "-b", "main", str(remote)], cwd=tmp)
    work = tmp / "work"
    _run(["git", "init", "-b", "main", str(work)], cwd=tmp)
    _run(["git", "remote", "add", "origin", str(remote)], cwd=work)
    _run(["git", "commit", "--allow-empty", "-m", "init"], cwd=work)
    _run(["git", "push", "-u", "origin", "main"], cwd=work)
    return work


def make_config(work: pathlib.Path, **overrides: Any) -> pathlib.Path:
    """Drop a minimal config/clean-merged.toml in the repo root."""
    cfg = work / "config" / "clean-merged.toml"
    cfg.parent.mkdir(parents=True, exist_ok=True)
    base = """
schema_version = 1
[clean-merged]
enabled = true
trunk_branch = "main"
remote_name = "origin"
origin_owner = "t"
[clean-merged.lane_r]
gh_timeout_s = 5
cache_ttl_s = 300
gh_limit = 100
[clean-merged.lane_w]
quarantine_dir = "<git-common-dir>/clean-merged-quarantine"
quarantine_grace_days = 30
discard_ignored = false
remove_nested_repos = false
discard_hidden_index_bits = false
archive_timeout_s = 120
archive_verify_timeout_s = 30
[clean-merged.logging]
audit_format = "jsonl"
audit_path = "<git-common-dir>/clean-merged.log"
max_log_bytes = 1048576
rotated_log_retention_days = 30
report_error_max_chars = 200
heartbeat_path = "<git-common-dir>/clean-merged.heartbeat"
heartbeat_stale_days = 7
lane_r_log_path = "<git-common-dir>/clean-merged.lane-r.log"
[clean-merged.backups]
prune_after_days = 30
"""
    cfg.write_text(base, encoding="utf-8")
    return cfg


def append_lane_t_config(
    work: pathlib.Path,
    *,
    idle_after_days: int = 7,
    active_process_patterns: tuple[str, ...] = ("cargo", "rustc"),
    process_list_timeout_s: float = 2,
    cwd_visibility_timeout_s: float = 1,
) -> None:
    cfg = work / "config" / "clean-merged.toml"
    patterns = ", ".join(json.dumps(pattern) for pattern in active_process_patterns)
    with cfg.open("a", encoding="utf-8") as handle:
        handle.write(
            textwrap.dedent(
                f"""\

                [clean-merged.lane_t]
                target_dir_name = "target"
                idle_after_days = {idle_after_days}
                active_process_patterns = [{patterns}]
                process_list_timeout_s = {process_list_timeout_s}
                cwd_visibility_timeout_s = {cwd_visibility_timeout_s}
                """
            )
        )


def run_clean(work: pathlib.Path, *args: str, env: dict[str, str] | None = None) -> int:
    """Subprocess runner — for end-to-end tests where mocking is not needed."""
    return run_clean_proc(work, *args, env=env).returncode


def run_clean_proc(
    work: pathlib.Path, *args: str, env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Subprocess runner that returns stdout/stderr for reporting assertions."""
    full_env = os.environ.copy()
    full_env.update(GIT_ENV)
    if env:
        full_env.update(env)
    return subprocess.run(
        [sys.executable, str(REPO_ROOT / "scripts" / "clean_merged_artifacts.py"), *args],
        cwd=work, env=full_env, capture_output=True, text=True, timeout=60,
    )


def run_clean_inproc(work: pathlib.Path, *args: str) -> int:
    """In-process runner — required for tests that patch `cm` module attributes
    (mocks do not cross subprocess boundaries). Runs cm.main() with cwd=work."""
    old_cwd = os.getcwd()
    old_env = os.environ.copy()
    os.environ.update(GIT_ENV)
    os.chdir(work)
    try:
        return cm.main(list(args))
    finally:
        os.chdir(old_cwd)
        os.environ.clear()
        os.environ.update(old_env)


def merged_branch_at(work: pathlib.Path, name: str, *, advance_to_trunk: bool = True) -> str:
    """Create a branch, advance it as an ancestor of main, return its tip SHA."""
    _run(["git", "branch", name], cwd=work)
    tip = _run(["git", "rev-parse", name], cwd=work).stdout.strip()
    return tip


def add_worktree(work: pathlib.Path, branch: str, dest: pathlib.Path) -> pathlib.Path:
    dest.parent.mkdir(parents=True, exist_ok=True)
    _run(["git", "worktree", "add", str(dest), branch], cwd=work)
    return dest


# ---------------------------------------------------------------------------
# Lane H tests


class LaneHTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-test-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_ancestor_deleted_via_lane_h(self) -> None:
        # Create a branch at current HEAD (so it's an ancestor of trunk after trunk advances)
        git(self.work, "branch", "feat/ancestor")
        # Advance trunk past the branch
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)
        # Now feat/ancestor is an ancestor of main
        rc = run_clean(self.work, "--lane", "h", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        branches = git(self.work, "branch", "--list")
        self.assertNotIn("feat/ancestor", branches)

    def test_unmerged_kept_by_lane_h(self) -> None:
        _run(["git", "checkout", "-b", "feat/unmerged"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "new-work"], cwd=self.work)
        _run(["git", "checkout", "main"], cwd=self.work)
        rc = run_clean(self.work, "--lane", "h", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        branches = git(self.work, "branch", "--list")
        self.assertIn("feat/unmerged", branches)

    def test_trunk_and_current_kept(self) -> None:
        _run(["git", "checkout", "-b", "feat/cur"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "x"], cwd=self.work)
        _run(["git", "checkout", "main"], cwd=self.work)
        # main is the trunk; feat/cur is unmerged -> kept anyway
        rc = run_clean(self.work, "--lane", "h", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        branches = git(self.work, "branch", "--list")
        self.assertIn("main", branches)
        self.assertIn("feat/cur", branches)

    def test_worktree_bound_skipped_by_lane_h(self) -> None:
        # branch at current HEAD, advanced trunk, but bound to a worktree
        _run(["git", "branch", "feat/bound"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)
        add_worktree(self.work, "feat/bound", self.tmp / "wt-bound")
        rc = run_clean(self.work, "--lane", "h", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        branches = git(self.work, "branch", "--list")
        self.assertIn("feat/bound", branches,
                       "Lane H must NOT touch worktree-bound branches (Lane W owns them)")

    def test_master_has_no_implicit_lane_h_protection(self) -> None:
        """Only configured trunk/current/keep are protected branch identities."""
        _run(["git", "branch", "master"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)

        rc = run_clean(self.work, "--lane", "h", "--apply", "--quiet")

        self.assertEqual(rc, 0)
        self.assertNotIn("master", git(self.work, "branch", "--list"))


# ---------------------------------------------------------------------------
# Lane R tests (gh mocked)


class LaneRTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-test-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _gh_mock_returning(self, payload: list[dict[str, Any]] | None,
                            error: str | None = None) -> Callable[..., Any]:
        def fake(repo_root: pathlib.Path, config: Any, branch: str, tip: str) -> tuple[Any, Any]:
            return payload, error
        return fake

    def test_headRefOid_mismatch_kept(self) -> None:
        # branch reused with new commits after a squash merge
        _run(["git", "checkout", "-b", "feat/x"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "old"], cwd=self.work)
        old_sha = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        # gh says merged at OLD sha
        _run(["git", "commit", "--allow-empty", "-m", "new-unmerged"], cwd=self.work)
        _run(["git", "checkout", "main"], cwd=self.work)
        # set the origin owner so the same-repo filter is exercised
        with mock.patch.object(cm, "gh_merged_pr_for_branch_cached",
                                self._gh_mock_returning([{
                                    "number": 1, "headRefOid": old_sha,
                                    "baseRefName": "main",
                                    "headRepositoryOwner": {"login": "t"},
                                    "isCrossRepository": False,
                                }])):
            rc = run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        branches = git(self.work, "branch", "--list")
        self.assertIn("feat/x", branches, "headRefOid mismatch must keep the branch")

    def test_baseRefName_mismatch_kept(self) -> None:
        _run(["git", "checkout", "-b", "feat/release"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "x"], cwd=self.work)
        tip = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "checkout", "main"], cwd=self.work)
        with mock.patch.object(cm, "gh_merged_pr_for_branch_cached",
                                self._gh_mock_returning([{
                                    "number": 2, "headRefOid": tip,
                                    "baseRefName": "release/1.0",  # not trunk
                                    "headRepositoryOwner": {"login": "t"},
                                    "isCrossRepository": False,
                                }])):
            rc = run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertIn("feat/release", git(self.work, "branch", "--list"))

    def test_cross_repo_pr_kept(self) -> None:
        _run(["git", "checkout", "-b", "feat/fork"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "x"], cwd=self.work)
        tip = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "checkout", "main"], cwd=self.work)
        with mock.patch.object(cm, "gh_merged_pr_for_branch_cached",
                                self._gh_mock_returning([{
                                    "number": 3, "headRefOid": tip,
                                    "baseRefName": "main",
                                    "headRepositoryOwner": {"login": "someone-else"},
                                    "isCrossRepository": True,
                                }])):
            rc = run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertIn("feat/fork", git(self.work, "branch", "--list"),
                       "cross-repo (fork) PR must not delete local branch")

    def test_exact_match_deleted_via_cas(self) -> None:
        _run(["git", "checkout", "-b", "feat/merged"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "x"], cwd=self.work)
        tip = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "checkout", "main"], cwd=self.work)
        with mock.patch.object(cm, "gh_merged_pr_for_branch_cached",
                                self._gh_mock_returning([{
                                    "number": 4, "headRefOid": tip,
                                    "baseRefName": "main",
                                    "headRepositoryOwner": {"login": "t"},
                                    "isCrossRepository": False,
                                }])):
            rc = run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertNotIn("feat/merged", git(self.work, "branch", "--list"))
        # backup ref should exist
        backups = git(self.work, "for-each-ref", "refs/clean-merged/")
        self.assertIn("feat/merged", backups)
        self.assertIn(tip[:12], backups)

    def test_gh_error_kept(self) -> None:
        _run(["git", "checkout", "-b", "feat/err"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "x"], cwd=self.work)
        _run(["git", "checkout", "main"], cwd=self.work)
        with mock.patch.object(cm, "gh_merged_pr_for_branch_cached",
                                self._gh_mock_returning(None, error="gh exit 1: auth")):
            rc = run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertIn("feat/err", git(self.work, "branch", "--list"),
                       "gh error must keep the branch (never treat trouble as merged)")

    def test_gh_timeout_kept(self) -> None:
        _run(["git", "checkout", "-b", "feat/timeout"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "x"], cwd=self.work)
        _run(["git", "checkout", "main"], cwd=self.work)
        with mock.patch.object(cm, "gh_merged_pr_for_branch_cached",
                                self._gh_mock_returning(None, error="gh timeout")):
            rc = run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertIn("feat/timeout", git(self.work, "branch", "--list"))

    def test_gh_missing_binary_kept(self) -> None:
        empty_bin = self.tmp / "empty-bin"
        empty_bin.mkdir()

        with mock.patch.dict(os.environ, {"PATH": str(empty_bin)}):
            prs, err = cm.gh_merged_pr_for_branch(self.work, "feat/missing-gh", 5, 100, 200)

        self.assertIsNone(prs)
        self.assertIsNotNone(err)
        self.assertIn("gh unavailable", err)

    def test_gh_query_only_requests_merged_prs(self) -> None:
        """Closed-unmerged PRs must not enter Lane R's merged-branch authority."""
        captured: dict[str, Any] = {}

        def fake_run(cmd: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
            captured["cmd"] = cmd
            return subprocess.CompletedProcess(cmd, 0, "[]", "")

        with mock.patch.object(cm.subprocess, "run", fake_run):
            prs, err = cm.gh_merged_pr_for_branch(self.work, "feat/closed", 5, 37, 200)

        self.assertEqual(prs, [])
        self.assertIsNone(err)
        cmd = captured["cmd"]
        state_idx = cmd.index("--state")
        self.assertEqual(cmd[state_idx + 1], "merged",
                         "closed-unmerged PRs must not be queried as cleanup authority")
        limit_idx = cmd.index("--limit")
        self.assertEqual(cmd[limit_idx + 1], "37",
                         "gh PR query limit must come from config, not a code literal")

    def test_gh_query_rejects_pr_payload_without_number(self) -> None:
        tip = "a" * 40

        def fake_run(cmd: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
            payload = [{
                "headRefOid": tip,
                "baseRefName": "main",
                "headRepositoryOwner": {"login": "t"},
                "isCrossRepository": False,
            }]
            return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

        with mock.patch.object(cm.subprocess, "run", fake_run):
            prs, err = cm.gh_merged_pr_for_branch(self.work, "feat/no-number", 5, 100, 200)

        self.assertIsNone(prs)
        self.assertIn("invalid PR payload", err or "")

    def test_lane_r_skips_worktree_bound(self) -> None:
        # The structural P0 fix: Lane R must NEVER update-ref -d a worktree-bound branch.
        _run(["git", "checkout", "-b", "feat/wt"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "x"], cwd=self.work)
        tip = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "checkout", "main"], cwd=self.work)
        add_worktree(self.work, "feat/wt", self.tmp / "wt-r")
        with mock.patch.object(cm, "gh_merged_pr_for_branch_cached",
                                self._gh_mock_returning([{
                                    "number": 5, "headRefOid": tip,
                                    "baseRefName": "main",
                                    "headRepositoryOwner": {"login": "t"},
                                    "isCrossRepository": False,
                                }])):
            rc = run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        # branch still exists, worktree still bound, NOT bricked
        self.assertIn("feat/wt", git(self.work, "branch", "--list"))
        # verify worktree HEAD is NOT zeros (not bricked)
        wt_head = _run(["git", "--git-dir", str(self.tmp / "wt-r" / ".git"),
                         "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        self.assertNotEqual(wt_head, "0" * 40, "Lane R must not brick the worktree")

    def test_branch_reuse_after_merge(self) -> None:
        # First commit (merged), second commit (unmerged) on same branch name.
        _run(["git", "checkout", "-b", "feat/reuse"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "v1"], cwd=self.work)
        v1 = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "commit", "--allow-empty", "-m", "v2-unmerged"], cwd=self.work)
        _run(["git", "checkout", "main"], cwd=self.work)
        # gh returns the merged PR for v1; current tip is v2 -> must mismatch -> kept
        with mock.patch.object(cm, "gh_merged_pr_for_branch_cached",
                                self._gh_mock_returning([{
                                    "number": 6, "headRefOid": v1,
                                    "baseRefName": "main",
                                    "headRepositoryOwner": {"login": "t"},
                                    "isCrossRepository": False,
                                }])):
            run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")
        self.assertIn("feat/reuse", git(self.work, "branch", "--list"))

    def test_master_has_no_implicit_lane_r_protection(self) -> None:
        """Lane R shares the same branch identity contract as Lane H."""
        _run(["git", "branch", "master"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)

        rc = run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")

        self.assertEqual(rc, 0)
        self.assertNotIn("master", git(self.work, "branch", "--list"))


# ---------------------------------------------------------------------------
# Lane W tests (the inversion)


class LaneWTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-test-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_purge_quarantine_preserves_dir_without_manifest(self) -> None:
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip()).resolve()
        q = common / "clean-merged-quarantine" / "missing-manifest"
        q.mkdir(parents=True)
        old_ts = time.time() - 31 * 86400
        os.utime(q, (old_ts, old_ts))

        purged = cm.cmd_purge_quarantine(
            self.work, cm.load_config(self.work), grace_days=30, quiet=True)

        self.assertEqual(purged, 0)
        self.assertTrue(q.exists(), "quarantine entries without manifest authority must be kept")

    def test_archive_timeouts_come_from_config(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8")
            .replace("archive_timeout_s = 120", "archive_timeout_s = 12")
            .replace("archive_verify_timeout_s = 30", "archive_verify_timeout_s = 3"),
            encoding="utf-8",
        )
        config = cm.load_config(self.work)
        calls: list[float] = []

        def fake_run(cmd: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
            calls.append(kwargs["timeout"])
            return subprocess.CompletedProcess(cmd, 0, "", "")

        with mock.patch.object(cm.subprocess, "run", fake_run):
            ok, err = cm._archive_worktree(self.work, self.tmp / "archive.tar.gz", config)

        self.assertTrue(ok, err)
        self.assertEqual(calls, [12, 3])

    def _setup_merged_worktree_branch(self, name: str = "feat/wt-merged",
                                       gitignore: str | None = None) -> pathlib.Path:
        # Branch at HEAD, then trunk advances so branch is an ancestor.
        # Optionally commit a .gitignore on trunk before branching so worktrees inherit it.
        if gitignore is not None:
            (self.work / ".gitignore").write_text(gitignore, encoding="utf-8")
            _run(["git", "add", ".gitignore"], cwd=self.work)
            _run(["git", "commit", "-m", "add gitignore"], cwd=self.work)
            _run(["git", "push", "-q", "origin", "main"], cwd=self.work)
        _run(["git", "branch", name], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)
        wt_path = self.tmp / f"wt-{name.replace('/', '-')}"
        add_worktree(self.work, name, wt_path)
        return wt_path

    def test_lane_w_owns_worktree_bound_branch(self) -> None:
        """The structural P0 fix: Lane W removes the worktree THEN deletes the branch."""
        wt_path = self._setup_merged_worktree_branch("feat/wt-merged")
        rc = run_clean(self.work, "--include-worktrees", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        # branch ref gone
        self.assertNotIn("feat/wt-merged", git(self.work, "branch", "--list"))
        # worktree removed from original location
        self.assertFalse(wt_path.exists(), "original worktree path must be vacated")
        # quarantine dir holds archive + manifest as one unit
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip()).resolve()
        q = common / "clean-merged-quarantine"
        self.assertTrue(q.is_dir())
        entries = list(q.iterdir())
        self.assertEqual(len(entries), 1, "one quarantine entry per worktree")
        quarantine_entry = entries[0]
        self.assertTrue((quarantine_entry / "worktree.tar.gz").is_file(),
                        "archive must live inside the quarantine dir")
        manifest = quarantine_entry / "clean-merged.manifest.json"
        self.assertTrue(manifest.is_file())
        m = json.loads(manifest.read_text())
        self.assertTrue(m["worktree_remove_ok"])

    def test_lane_w_refuses_ignored_content(self) -> None:
        wt_path = self._setup_merged_worktree_branch(
            "feat/ignored", gitignore="*.env\n")
        # add an ignored file (matches the committed .gitignore on trunk)
        (wt_path / "secret.env").write_text("KEY=1\n", encoding="utf-8")
        rc = run_clean(self.work, "--include-worktrees", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        # worktree still there, branch still there
        self.assertTrue(wt_path.exists(), "ignored content must block removal")
        self.assertIn("feat/ignored", git(self.work, "branch", "--list"))

    def test_discard_ignored_override(self) -> None:
        wt_path = self._setup_merged_worktree_branch(
            "feat/disc", gitignore="*.env\n")
        (wt_path / "secret.env").write_text("KEY=1\n", encoding="utf-8")
        rc = run_clean(self.work, "--include-worktrees", "--apply",
                        "--discard-ignored", "--quiet")
        self.assertEqual(rc, 0)
        self.assertFalse(wt_path.exists())
        self.assertNotIn("feat/disc", git(self.work, "branch", "--list"))

    def _setup_merged_worktree_with_tracked_file(
        self, name: str, tracked_filename: str = "tracked.txt",
    ) -> tuple[pathlib.Path, pathlib.Path]:
        """Set up an eligible (ancestor-of-trunk) worktree bound to `name`,
        with a tracked file already committed on trunk. Returns (wt_path, file_path).

        Used by the hidden-index-bits tests so the guard is the actual refusal
        point (the branch IS eligible), not branch ineligibility.
        """
        # Commit tracked.txt on trunk first so it inherits to the worktree.
        (self.work / tracked_filename).write_text("v1\n", encoding="utf-8")
        _run(["git", "add", tracked_filename], cwd=self.work)
        _run(["git", "commit", "-m", "add tracked"], cwd=self.work)
        _run(["git", "push", "-q", "origin", "main"], cwd=self.work)
        # Branch at this HEAD; then advance trunk so branch is an ancestor.
        _run(["git", "branch", name], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)
        wt_path = self.tmp / f"wt-{name.replace('/', '-')}"
        add_worktree(self.work, name, wt_path)
        return wt_path, wt_path / tracked_filename

    def test_assume_unchanged_refused(self) -> None:
        wt_path, tracked = self._setup_merged_worktree_with_tracked_file("feat/au")
        # Modify the tracked file in the worktree, then hide the modification.
        tracked.write_text("modified-but-hidden\n", encoding="utf-8")
        _run(["git", "update-index", "--assume-unchanged", tracked.name], cwd=wt_path)
        rc = run_clean(self.work, "--include-worktrees", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        # refused: worktree + branch still present
        self.assertTrue(wt_path.exists())
        self.assertIn("feat/au", git(self.work, "branch", "--list"))
        # And specifically refused for the hidden-bits reason (not dirty/other):
        # git status --porcelain would be empty (the bit hides the modification),
        # so without the guard the worktree would have been removed.

    def test_skip_worktree_refused(self) -> None:
        wt_path, tracked = self._setup_merged_worktree_with_tracked_file("feat/sw")
        tracked.write_text("hidden\n", encoding="utf-8")
        _run(["git", "update-index", "--skip-worktree", tracked.name], cwd=wt_path)
        rc = run_clean(self.work, "--include-worktrees", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertTrue(wt_path.exists(),
                        "skip-worktree (uppercase S in ls-files -v) must block removal")
        self.assertIn("feat/sw", git(self.work, "branch", "--list"))

    def test_dirty_worktree_refused(self) -> None:
        wt_path = self._setup_merged_worktree_branch("feat/dirty")
        (wt_path / "tracked.txt").write_text("dirty\n", encoding="utf-8")
        _run(["git", "add", "tracked.txt"], cwd=wt_path)
        proc = run_clean_proc(self.work, "--include-worktrees", "--apply")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(wt_path.exists())
        self.assertIn("feat/dirty", git(self.work, "branch", "--list"))
        self.assertIn("refused-dirty", proc.stdout)

    def test_nested_git_refused(self) -> None:
        wt_path = self._setup_merged_worktree_branch("feat/nested")
        nested = wt_path / "vendor" / "ext"
        nested.mkdir(parents=True)
        _run(["git", "init"], cwd=nested)
        rc = run_clean(self.work, "--include-worktrees", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertTrue(wt_path.exists())
        self.assertIn("feat/nested", git(self.work, "branch", "--list"))

    def test_lane_w_branch_ref_exists_during_eligibility(self) -> None:
        """Verify the inversion: the branch ref must still exist when Lane W runs,
        so its eligibility check can use it. We assert by intercepting gh calls."""
        wt_path = self._setup_merged_worktree_branch("feat/ref-exists")
        # When Lane W runs, the ref must exist; capture state at eligibility time
        original = cm._lane_w_eligible
        captured: dict[str, Any] = {}

        def spy(repo_root, config, *, branch, head, trunk_sha, **kwargs):
            refs = git(repo_root, "for-each-ref", f"refs/heads/{branch}")
            captured["ref_present"] = branch in refs
            return original(repo_root, config, branch=branch, head=head, trunk_sha=trunk_sha, **kwargs)

        with mock.patch.object(cm, "_lane_w_eligible", spy):
            rc = run_clean_inproc(self.work, "--include-worktrees", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertTrue(captured.get("ref_present"),
                        "Lane W eligibility must run BEFORE the branch ref is deleted")

    def test_lane_w_refuses_branch_delete_when_tip_drifts_to_unmerged_commit(self) -> None:
        wt_path = self._setup_merged_worktree_branch("feat/tip-drift")
        original_git = cm._git
        injected: dict[str, str] = {}

        def inject_clean_commit_before_remove(
            repo_root: pathlib.Path, args: list[str], **kwargs: Any,
        ) -> subprocess.CompletedProcess[str]:
            if args[:2] == ["worktree", "remove"] and "sha" not in injected:
                (wt_path / "late.txt").write_text("late local work\n", encoding="utf-8")
                _run(["git", "add", "late.txt"], cwd=wt_path)
                _run(["git", "commit", "-m", "late local work"], cwd=wt_path)
                injected["sha"] = git(wt_path, "rev-parse", "HEAD").strip()
            return original_git(repo_root, args, **kwargs)

        with mock.patch.object(cm, "_git", inject_clean_commit_before_remove):
            rc = run_clean_inproc(self.work, "--include-worktrees", "--apply", "--quiet")

        self.assertEqual(rc, 0)
        self.assertIn("sha", injected, "test must inject a post-eligibility branch tip drift")
        self.assertFalse(wt_path.exists(), "clean worktree removal may still complete")
        self.assertIn("feat/tip-drift", git(self.work, "branch", "--list"),
                      "fresh unmerged branch tip must remain reachable by branch name")
        self.assertEqual(git(self.work, "rev-parse", "feat/tip-drift").strip(),
                         injected["sha"],
                         "branch must point at the fresh unmerged tip")

    def test_master_has_no_implicit_lane_w_protection(self) -> None:
        """Lane W may remove a worktree-bound branch named master when eligible."""
        wt_path = self._setup_merged_worktree_branch("master")

        rc = run_clean(self.work, "--include-worktrees", "--apply", "--quiet")

        self.assertEqual(rc, 0)
        self.assertFalse(wt_path.exists())
        self.assertNotIn("master", git(self.work, "branch", "--list"))

    def test_clean_merged_worktree_reports_removal_in_dry_run(self) -> None:
        wt_path = self._setup_merged_worktree_branch("feat/wt-dry-run")
        proc = run_clean_proc(self.work, "--include-worktrees")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(wt_path.exists(), "dry-run must not remove the worktree")
        self.assertIn("feat/wt-dry-run", git(self.work, "branch", "--list"))
        self.assertIn("would-archive-and-remove", proc.stdout)

    def test_lane_w_iteration_exception_audit_type_error_does_not_kill_sweep(self) -> None:
        self._setup_merged_worktree_branch("feat/audit-typeerror")
        config = cm.load_config(self.work)

        with mock.patch.object(cm, "_lane_w_eligible",
                               side_effect=RuntimeError("probe failure")):
            with mock.patch.object(cm, "write_audit",
                                   side_effect=TypeError("not json serializable")):
                records = cm.run_lane_w(self.work, config, apply=False, keep=set(),
                                        quiet=True, discard_ignored=False,
                                        remove_nested=False, discard_hidden=False)

        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]["action"], "iteration-exception")
        self.assertIn("RuntimeError: probe failure", records[0]["reason"])


# ---------------------------------------------------------------------------
# Cross-cutting / infra tests


class InfraTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-test-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_dry_run_mutates_nothing(self) -> None:
        # feat/keep at current HEAD; trunk then advances so feat/keep IS an ancestor.
        # Dry-run must report it as a candidate but NOT delete.
        _run(["git", "branch", "feat/keep"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)
        before = git(self.work, "for-each-ref")
        rc = run_clean(self.work, "--lane", "h")  # no --apply
        self.assertEqual(rc, 0)
        after = git(self.work, "for-each-ref")
        self.assertEqual(before, after, "dry-run must not mutate refs")
        # backup ref must NOT exist
        self.assertEqual(git(self.work, "for-each-ref", "refs/clean-merged/").strip(), "")

    def test_kill_switch(self) -> None:
        _run(["git", "branch", "feat/kill"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)
        rc = run_clean(self.work, "--lane", "h", "--apply", "--quiet",
                        env={"CLEAN_MERGED_DISABLED": "1"})
        self.assertEqual(rc, 0)
        self.assertIn("feat/kill", git(self.work, "branch", "--list"))

    def test_audit_log_under_git_common_dir(self) -> None:
        # Delete a branch via Lane H, then verify the audit log lands in
        # git-common-dir (NOT a per-worktree .git), which matters for linked worktrees.
        _run(["git", "branch", "feat/audit"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)
        rc = run_clean(self.work, "--lane", "h", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertNotIn("feat/audit", git(self.work, "branch", "--list"),
                          "precondition: feat/audit was eligible and should have been deleted")
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip()).resolve()
        log = common / "clean-merged.log"
        self.assertTrue(log.is_file(), "audit log must be written under git-common-dir")
        content = log.read_text(encoding="utf-8")
        # JSONL record with the deleted branch
        self.assertIn("feat/audit", content)
        self.assertIn("\"lane\"", content)
        # specifically NOT under a per-worktree .git/log path
        self.assertTrue(log.parent.samefile(common))

    def test_backup_ref_is_sha_addressed(self) -> None:
        _run(["git", "checkout", "-b", "feat/sha"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "x"], cwd=self.work)
        tip = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "checkout", "main"], cwd=self.work)
        with mock.patch.object(cm, "gh_merged_pr_for_branch_cached",
                                lambda r, c, b, t: ([{
                                    "number": 9, "headRefOid": tip, "baseRefName": "main",
                                    "headRepositoryOwner": {"login": "t"},
                                    "isCrossRepository": False,
                                }], None)):
            run_clean_inproc(self.work, "--lane", "r", "--apply", "--quiet")
        backups = git(self.work, "for-each-ref", "--format=%(refname)",
                       "refs/clean-merged/")
        self.assertIn(tip[:12], backups,
                       "backup ref must include the tip SHA (SHA-addressed for reuse-safety)")

    def test_doctor_runs(self) -> None:
        # doctor must not crash even when hooks aren't installed
        rc = run_clean(self.work, "--doctor")
        # returns 1 if problems found (hooks not installed in test env), 0 if all green;
        # both are acceptable; we only assert it runs to completion
        self.assertIn(rc, (0, 1))

    def test_doctor_reports_missing_gh_without_traceback(self) -> None:
        git_bin = shutil.which("git")
        self.assertIsNotNone(git_bin, "git must be available for the test suite")
        bin_dir = self.tmp / "git-only-bin"
        bin_dir.mkdir()
        (bin_dir / "git").symlink_to(git_bin)

        proc = run_clean_proc(self.work, "--doctor", env={"PATH": str(bin_dir)})

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("gh available             = False", proc.stdout)
        self.assertIn("gh CLI not available; Lane R cannot run", proc.stdout)
        self.assertNotIn("Traceback", proc.stdout + proc.stderr)

    def test_doctor_reports_rotated_log_usage(self) -> None:
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip())
        (common / "clean-merged.log.1").write_text("audit\n", encoding="utf-8")
        (common / "clean-merged.lane-r.log.1.123.456").write_text("lane r\n", encoding="utf-8")

        proc = run_clean_proc(self.work, "--doctor")

        self.assertIn("rotated logs             = 2 files", proc.stdout)

    def test_doctor_heartbeat_stale_threshold_comes_from_config(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8").replace(
                "heartbeat_stale_days = 7",
                "heartbeat_stale_days = 1",
            ),
            encoding="utf-8",
        )
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip())
        hb = common / "clean-merged.heartbeat"
        hb.write_text(
            (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=2)).isoformat(),
            encoding="utf-8",
        )

        proc = run_clean_proc(self.work, "--doctor")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("heartbeat stale", proc.stdout)


# ---------------------------------------------------------------------------
# Remote sync tests (#1050)


class SyncMainTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-sync-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)
        _run(["git", "add", "config/clean-merged.toml"], cwd=self.work)
        _run(["git", "commit", "-m", "add clean-merged config"], cwd=self.work)
        _run(["git", "push", "-q", "origin", "main"], cwd=self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _advance_remote_main(self, message: str = "remote-advance") -> str:
        other = self.tmp / f"other-{time.time_ns()}"
        _run(["git", "clone", "-q", "-b", "main", str(self.tmp / "remote.git"), str(other)],
             cwd=self.tmp)
        _run(["git", "commit", "--allow-empty", "-m", message], cwd=other)
        _run(["git", "push", "-q", "origin", "main"], cwd=other)
        return _run(["git", "rev-parse", "HEAD"], cwd=other).stdout.strip()

    def test_sync_main_dry_run_changes_nothing(self) -> None:
        self._advance_remote_main()
        before = git(self.work, "for-each-ref", "--format=%(refname) %(objectname)")

        proc = run_clean_proc(self.work, "--sync-main")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        after = git(self.work, "for-each-ref", "--format=%(refname) %(objectname)")
        self.assertEqual(before, after, "sync dry-run must not mutate any refs")
        self.assertIn("would-fetch-prune", proc.stdout)

    def test_sync_main_dry_run_does_not_claim_stale_tracking_ref_is_current(self) -> None:
        self._advance_remote_main()

        proc = run_clean_proc(self.work, "--sync-main")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("would-evaluate-after-fetch", proc.stdout)
        self.assertNotIn("tracking-ref-up-to-date", proc.stdout)

    def test_sync_main_dry_run_reports_cleanup_after_preview_sync(self) -> None:
        _run(["git", "checkout", "-b", "feat/preview-w"], cwd=self.work)
        (self.work / "preview.txt").write_text("merged\n", encoding="utf-8")
        _run(["git", "add", "preview.txt"], cwd=self.work)
        _run(["git", "commit", "-m", "preview feature"], cwd=self.work)
        _run(["git", "push", "-u", "origin", "feat/preview-w"], cwd=self.work)
        _run(["git", "checkout", "main"], cwd=self.work)
        wt_path = self.tmp / "wt-preview-w"
        add_worktree(self.work, "feat/preview-w", wt_path)
        before_refs = git(self.work, "for-each-ref", "--format=%(refname) %(objectname)")

        other = self.tmp / "other-preview-merge"
        _run(["git", "clone", "-q", "-b", "main", str(self.tmp / "remote.git"), str(other)],
             cwd=self.tmp)
        _run(["git", "fetch", "-q", "origin", "feat/preview-w:feat/preview-w"], cwd=other)
        _run(["git", "merge", "--ff-only", "feat/preview-w"], cwd=other)
        _run(["git", "push", "-q", "origin", "main"], cwd=other)

        proc = run_clean_proc(self.work, "--sync-main", "--include-worktrees")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(wt_path.exists(), "dry-run must not remove worktrees")
        self.assertEqual(git(self.work, "for-each-ref", "--format=%(refname) %(objectname)"),
                         before_refs,
                         "preview sync dry-run must not mutate refs")
        self.assertIn("preview-fetched-trunk", proc.stdout)
        self.assertIn("lane W dry-run", proc.stdout)
        self.assertIn("would-archive-and-remove", proc.stdout)

    def test_sync_main_dry_run_refuses_when_preview_temp_ref_delete_fails(self) -> None:
        _run(["git", "branch", "feat/direct-cleanup-refusal"], cwd=self.work)
        wt_path = self.tmp / "wt-direct-cleanup-refusal"
        add_worktree(self.work, "feat/direct-cleanup-refusal", wt_path)
        self._advance_remote_main()
        original_git = cm._git

        def fail_preview_ref_delete(
            repo_root: pathlib.Path, args: list[str], **kwargs: Any,
        ) -> subprocess.CompletedProcess[str]:
            if (args[:2] == ["update-ref", "-d"]
                    and len(args) > 2
                    and args[2].startswith("refs/clean-merged-preview/")):
                return subprocess.CompletedProcess(args, 1, "", "cannot delete preview ref")
            return original_git(repo_root, args, **kwargs)

        stdout = io.StringIO()
        stderr = io.StringIO()
        with mock.patch.object(cm, "_git", fail_preview_ref_delete):
            with contextlib.redirect_stdout(stdout):
                with contextlib.redirect_stderr(stderr):
                    rc = run_clean_inproc(self.work, "--sync-main", "--include-worktrees")

        self.assertEqual(rc, 0)
        self.assertTrue(wt_path.exists(), "cleanup refusal must stop worktree preview")
        self.assertIn("preview-ref-cleanup-failed", stdout.getvalue())
        self.assertIn("usable cleanup authority", stderr.getvalue())
        self.assertNotIn("lane W dry-run", stdout.getvalue())
        self.assertNotIn("would-archive-and-remove", stdout.getvalue())

    def test_sync_main_dry_run_refuses_when_preview_temp_ref_cannot_be_deleted(self) -> None:
        _run(["git", "branch", "feat/cleanup-refusal"], cwd=self.work)
        wt_path = self.tmp / "wt-cleanup-refusal"
        add_worktree(self.work, "feat/cleanup-refusal", wt_path)
        self._advance_remote_main()
        original_git = cm._git

        def fail_all_preview_ref_deletes(
            repo_root: pathlib.Path, args: list[str], **kwargs: Any,
        ) -> subprocess.CompletedProcess[str]:
            if (args[:2] == ["update-ref", "-d"]
                    and len(args) > 2
                    and args[2].startswith("refs/clean-merged-preview/")):
                return subprocess.CompletedProcess(args, 1, "", "cannot delete preview ref")
            return original_git(repo_root, args, **kwargs)

        stdout = io.StringIO()
        stderr = io.StringIO()
        with mock.patch.object(cm, "_git", fail_all_preview_ref_deletes):
            with contextlib.redirect_stdout(stdout):
                with contextlib.redirect_stderr(stderr):
                    rc = run_clean_inproc(self.work, "--sync-main", "--include-worktrees")

        self.assertEqual(rc, 0)
        self.assertTrue(wt_path.exists(), "cleanup refusal must stop worktree preview")
        self.assertIn("preview-ref-cleanup-failed", stdout.getvalue())
        self.assertIn("usable cleanup authority", stderr.getvalue())
        self.assertNotIn("fast-forward-safe trunk", stderr.getvalue())
        self.assertNotIn("lane W dry-run", stdout.getvalue())
        self.assertNotIn("would-archive-and-remove", stdout.getvalue())

    def test_sync_main_dry_run_refuses_non_fast_forward_before_cleanup_preview(self) -> None:
        _run(["git", "branch", "feat/nonff-preview"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "local-only"], cwd=self.work)
        local_sha = git(self.work, "rev-parse", "main").strip()
        wt_path = self.tmp / "wt-nonff-preview"
        add_worktree(self.work, "feat/nonff-preview", wt_path)
        self._advance_remote_main()

        proc = run_clean_proc(self.work, "--sync-main", "--include-worktrees")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(git(self.work, "rev-parse", "main").strip(), local_sha)
        self.assertTrue(wt_path.exists(), "dry-run must not remove worktrees")
        self.assertIn("refused-non-fast-forward", proc.stdout)
        self.assertNotIn("lane W dry-run", proc.stdout)
        self.assertNotIn("would-archive-and-remove", proc.stdout)

    def test_sync_main_dry_run_refuses_dirty_checked_out_main_before_cleanup_preview(self) -> None:
        _run(["git", "branch", "feat/dirty-preview"], cwd=self.work)
        wt_path = self.tmp / "wt-dirty-preview"
        add_worktree(self.work, "feat/dirty-preview", wt_path)
        self._advance_remote_main()
        local_sha = git(self.work, "rev-parse", "main").strip()
        (self.work / "operator-note.txt").write_text("dirty main\n", encoding="utf-8")

        proc = run_clean_proc(self.work, "--sync-main", "--include-worktrees")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(git(self.work, "rev-parse", "main").strip(), local_sha)
        self.assertTrue(wt_path.exists(), "dry-run must not remove worktrees")
        self.assertIn("refused-dirty-trunk-worktree", proc.stdout)
        self.assertNotIn("lane W dry-run", proc.stdout)
        self.assertNotIn("would-archive-and-remove", proc.stdout)

    def test_sync_main_apply_fast_forwards_local_main(self) -> None:
        remote_sha = self._advance_remote_main()

        proc = run_clean_proc(self.work, "--sync-main", "--apply", "--quiet")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(git(self.work, "rev-parse", "main").strip(), remote_sha)
        self.assertEqual(git(self.work, "rev-parse", "refs/remotes/origin/main").strip(),
                         remote_sha)

    def test_sync_main_apply_updates_unchecked_out_main_ref(self) -> None:
        remote_sha = self._advance_remote_main()
        _run(["git", "checkout", "-b", "operator"], cwd=self.work)
        operator_sha = git(self.work, "rev-parse", "HEAD").strip()

        proc = run_clean_proc(self.work, "--sync-main", "--apply", "--quiet")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(git(self.work, "branch", "--show-current").strip(), "operator")
        self.assertEqual(git(self.work, "rev-parse", "HEAD").strip(), operator_sha)
        self.assertEqual(git(self.work, "rev-parse", "main").strip(), remote_sha)

    def test_sync_main_apply_refuses_non_fast_forward(self) -> None:
        _run(["git", "commit", "--allow-empty", "-m", "local-only"], cwd=self.work)
        local_sha = git(self.work, "rev-parse", "main").strip()
        self._advance_remote_main()

        proc = run_clean_proc(self.work, "--sync-main", "--apply")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(git(self.work, "rev-parse", "main").strip(), local_sha)
        self.assertIn("refused-non-fast-forward", proc.stdout)

    def test_sync_main_apply_refusal_stops_cleanup_lanes(self) -> None:
        _run(["git", "branch", "feat/blocked-w"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "local-only"], cwd=self.work)
        wt_path = self.tmp / "wt-blocked-w"
        add_worktree(self.work, "feat/blocked-w", wt_path)
        self._advance_remote_main()

        proc = run_clean_proc(self.work, "--sync-main", "--include-worktrees", "--apply")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(wt_path.exists(), "Lane W must not run after Lane S refusal")
        self.assertIn("feat/blocked-w", git(self.work, "branch", "--list"))
        self.assertIn("refused-non-fast-forward", proc.stdout)
        self.assertIn("cleanup skipped because lane S", proc.stderr)

    def test_sync_main_apply_refuses_dirty_checked_out_main(self) -> None:
        self._advance_remote_main()
        local_sha = git(self.work, "rev-parse", "main").strip()
        (self.work / "local-note.txt").write_text("operator scratch\n", encoding="utf-8")

        proc = run_clean_proc(self.work, "--sync-main", "--apply")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(git(self.work, "rev-parse", "main").strip(), local_sha)
        self.assertIn("refused-dirty-trunk-worktree", proc.stdout)

    def test_remote_branch_gone_but_merged_to_main_is_cleaned_after_sync(self) -> None:
        _run(["git", "checkout", "-b", "feat/gone-merged"], cwd=self.work)
        (self.work / "feature.txt").write_text("merged\n", encoding="utf-8")
        _run(["git", "add", "feature.txt"], cwd=self.work)
        _run(["git", "commit", "-m", "feature"], cwd=self.work)
        feature_sha = git(self.work, "rev-parse", "HEAD").strip()
        _run(["git", "push", "-u", "origin", "feat/gone-merged"], cwd=self.work)
        _run(["git", "checkout", "main"], cwd=self.work)

        other = self.tmp / "other-merge"
        _run(["git", "clone", "-q", "-b", "main", str(self.tmp / "remote.git"), str(other)],
             cwd=self.tmp)
        _run(["git", "fetch", "-q", "origin", "feat/gone-merged:feat/gone-merged"],
             cwd=other)
        _run(["git", "merge", "--ff-only", "feat/gone-merged"], cwd=other)
        _run(["git", "push", "-q", "origin", "main"], cwd=other)
        _run(["git", "push", "-q", "origin", "--delete", "feat/gone-merged"], cwd=other)

        proc = run_clean_proc(self.work, "--sync-main", "--apply", "--quiet")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(git(self.work, "rev-parse", "main").strip(), feature_sha)
        self.assertNotEqual(
            _run(["git", "rev-parse", "--verify", "refs/remotes/origin/feat/gone-merged"],
                 cwd=self.work, check=False).returncode,
            0,
            "fetch --prune must remove the stale remote-tracking branch",
        )
        self.assertNotIn("feat/gone-merged", git(self.work, "branch", "--list"),
                         "local branch merged into refreshed main is safe to delete")


# ---------------------------------------------------------------------------
# Report redaction tests


class ReportRedactionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-redact-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)
        self.config = cm.load_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_fetch_failure_reason_redacts_url_credentials(self) -> None:
        raw = ("fatal: unable to access "
               "'https://user:secret-token@example.invalid/private.git/': denied")

        def fake_git(
            repo_root: pathlib.Path, args: list[str], **kwargs: Any,
        ) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(args, 128, "", raw)

        with mock.patch.object(cm, "_git", fake_git):
            record = cm._sync_fetch_record(self.work, self.config, apply=True)

        self.assertEqual(record["action"], "fetch-prune-failed")
        self.assertNotIn("secret-token", record["reason"])
        self.assertNotIn("user:secret-token", record["reason"])
        self.assertIn("<redacted>", record["reason"])

    def test_fetch_failure_reason_uses_configured_report_limit(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8").replace(
                "report_error_max_chars = 200", "report_error_max_chars = 12"),
            encoding="utf-8",
        )
        config = cm.load_config(self.work)

        def fake_git(
            repo_root: pathlib.Path, args: list[str], **kwargs: Any,
        ) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(
                args, 128, "", "fatal: abcdefghijklmnopqrstuvwxyz")

        with mock.patch.object(cm, "_git", fake_git):
            record = cm._sync_fetch_record(self.work, config, apply=True)

        self.assertLessEqual(len(record["reason"]), 12)

    def test_report_error_redacts_common_secret_forms(self) -> None:
        raw = (
            "Authorization: Bearer ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa "
            "token=secret-token password: hunter2 "
            "github_pat_11AAAAAAAAAAAAAAAAAAAA_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )

        safe = cm._safe_report_error(raw, limit=self.config.logging.report_error_max_chars)

        self.assertNotIn("ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", safe)
        self.assertNotIn("secret-token", safe)
        self.assertNotIn("hunter2", safe)
        self.assertNotIn("github_pat_11AAAAAAAAAAAAAAAAAAAA", safe)
        self.assertIn("<redacted>", safe)

    def test_gh_error_is_redacted_without_cache_write(self) -> None:
        branch = "feat/gh-error"
        _run(["git", "branch", branch], cwd=self.work)
        tip = git(self.work, "rev-parse", branch).strip()
        raw_error = "gh exit 1: https://user:secret-token@example.invalid/private.git"

        with mock.patch.object(cm, "gh_merged_pr_for_branch", return_value=(None, raw_error)):
            prs, err = cm.gh_merged_pr_for_branch_cached(self.work, self.config, branch, tip)

        self.assertIsNone(prs)
        self.assertIsNotNone(err)
        assert err is not None
        self.assertNotIn("secret-token", err)
        self.assertNotIn("user:secret-token", err)
        self.assertIn("<redacted>", err)
        cache_path = cm._gh_cache_path(self.work)
        if cache_path.exists():
            self.assertNotIn("secret-token", cache_path.read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# Clean-merged contract tests


class CleanupContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-contract-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _install_post_rewrite_with_clean_marker(self) -> tuple[pathlib.Path, pathlib.Path]:
        hook = self.work / ".git" / "hooks" / "post-rewrite"
        shutil.copy2(REPO_ROOT / ".githooks" / "post-rewrite", hook)

        marker = self.tmp / "clean-merged-dispatched"
        script = self.work / "scripts" / "clean_merged_artifacts.py"
        script.parent.mkdir(parents=True, exist_ok=True)
        script.write_text(
            "#!/usr/bin/env python3\n"
            "import pathlib\n"
            f"pathlib.Path({json.dumps(str(marker))}).write_text("
            "'dispatched', encoding='utf-8')\n",
            encoding="utf-8",
        )
        return hook, marker

    def _write_clean_merged_hook_sources(
        self,
        root: pathlib.Path,
        *,
        label: str = "source",
        track: bool = True,
    ) -> pathlib.Path:
        source_hooks = root / ".githooks"
        source_hooks.mkdir(exist_ok=True)
        for hook in cm.CLEAN_MERGED_HOOKS:
            hook_file = source_hooks / hook
            hook_file.write_text(
                f"#!/bin/sh\n# clean-merged-managed\nprintf {label}-{hook}\n",
                encoding="utf-8",
            )
            hook_file.chmod(0o755)
        if track:
            _run(["git", "add", ".githooks"], cwd=root)
            _run(["git", "commit", "-m", f"track {label} hook sources"], cwd=root)
        return source_hooks

    def test_lane_summary_prints_full_refusal_reason(self) -> None:
        reason = (
            "detached-HEAD worktree refused "
            "(use --allow-detached-removal to override; "
            "reflog-only commits are not preserved by the archive)"
        )
        stdout = io.StringIO()

        with contextlib.redirect_stdout(stdout):
            cm._print_lane_summary("W", [{
                "action": "refused-detached-head",
                "branch": "detached-5a4127b5",
                "tip_sha": "5a4127b52a7f",
                "worktree": "/tmp/wt",
                "reason": reason,
            }], apply=False)

        out = stdout.getvalue()
        self.assertIn(reason, out)
        self.assertIn("reflog-only commits are not preserved by the archive)", out)

    def test_no_alternate_toml_parser_path(self) -> None:
        source = (REPO_ROOT / "scripts" / "clean_merged_artifacts.py").read_text(
            encoding="utf-8")
        for token in ("_parse_toml_flat", "_HAS_TOMLLIB", "fallback flat parser"):
            self.assertNotIn(token, source)

    def test_no_rule_engine_classifier_path(self) -> None:
        source = (REPO_ROOT / "scripts" / "clean_merged_artifacts.py").read_text(
            encoding="utf-8")
        for token in ("ContractRule", "PrPayloadRule", "_classify_contract"):
            self.assertNotIn(token, source)

    def test_hook_ownership_does_not_parse_hook_body_markers(self) -> None:
        source = (REPO_ROOT / "scripts" / "clean_merged_artifacts.py").read_text(
            encoding="utf-8")
        for token in ("HOOK_MARKER", "_hook_is_managed", "not marked managed"):
            self.assertNotIn(token, source)

    def test_install_hooks_uses_plan_for_hook_runtime_mutations(self) -> None:
        source = (REPO_ROOT / "scripts" / "clean_merged_artifacts.py").read_text(
            encoding="utf-8")
        module = ast.parse(source)
        install_hooks = next(
            node for node in module.body
            if isinstance(node, ast.FunctionDef) and node.name == "install_hooks"
        )
        disallowed_calls = {
            "_copy_hook_with_provenance",
            "_remove_hook_with_provenance",
            "_shadowed_hook_copy",
            "_set_runtime_hooks_path",
            "_write_hook_manifest",
            "unlink",
            "chmod",
        }
        found: list[str] = []
        for node in ast.walk(install_hooks):
            if not isinstance(node, ast.Call):
                continue
            call = node.func
            if isinstance(call, ast.Name) and call.id in disallowed_calls:
                found.append(call.id)
            if isinstance(call, ast.Attribute) and call.attr in disallowed_calls:
                found.append(call.attr)
            if (
                isinstance(call, ast.Attribute)
                and call.attr == "copy2"
                and isinstance(call.value, ast.Name)
                and call.value.id == "shutil"
            ):
                found.append("shutil.copy2")
        self.assertEqual(found, [])

    def test_install_hooks_revalidates_plan_before_runtime_mutation(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        hook_source = self.work / ".githooks" / "post-merge"
        runtime_hooks_dir = git_common_dir_compat(self.work) / "hooks"
        original_apply = cm.HookInstallPlan.apply

        def mutate_source_before_apply(plan: cm.HookInstallPlan) -> None:
            hook_source.write_text(
                "#!/bin/sh\nprintf changed-during-plan\n",
                encoding="utf-8",
            )
            hook_source.chmod(0o755)
            original_apply(plan)

        with mock.patch.dict(os.environ, GIT_ENV, clear=False):
            with mock.patch.object(
                cm.HookInstallPlan,
                "apply",
                mutate_source_before_apply,
            ):
                with self.assertRaisesRegex(
                    cm.CleanMergedError,
                    "tracked hook source\\(s\\) have local changes",
                ):
                    cm.install_hooks(self.work, home_dir=pathlib.Path(GIT_ENV["HOME"]))

        self.assertFalse((runtime_hooks_dir / "post-merge").exists())
        manifest = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        self.assertFalse(manifest.exists())
        config = _run(["git", "config", "--get", "core.hooksPath"],
                      cwd=self.work, check=False)
        self.assertNotEqual(config.returncode, 0)

    def test_install_hooks_copies_planned_bytes_after_preflight_window(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        hook_source = self.work / ".githooks" / "post-merge"
        planned_content = hook_source.read_text(encoding="utf-8")

        def mutate_source_after_preflight(plan: cm.HookInstallPlan) -> None:
            for operation in plan.preflight_operations:
                operation()
            hook_source.write_text(
                "#!/bin/sh\nprintf changed-after-preflight\n",
                encoding="utf-8",
            )
            hook_source.chmod(0o755)
            for operation in plan.operations:
                operation()

        with mock.patch.dict(os.environ, GIT_ENV, clear=False):
            with mock.patch.object(
                cm.HookInstallPlan,
                "apply",
                mutate_source_after_preflight,
            ):
                runtime_hooks_dir = cm.install_hooks(
                    self.work,
                    home_dir=pathlib.Path(GIT_ENV["HOME"]),
                )

        runtime_hook = runtime_hooks_dir / "post-merge"
        self.assertEqual(runtime_hook.read_text(encoding="utf-8"), planned_content)
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        runtime_sha = file_sha256(runtime_hook)
        self.assertEqual(manifest["hooks"]["post-merge"]["source_sha256"], runtime_sha)
        self.assertEqual(manifest["hooks"]["post-merge"]["runtime_sha256"], runtime_sha)

    def test_install_hooks_does_not_install_repo_source_dirty_after_dirty_check(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        hook_source = self.work / ".githooks" / "post-merge"
        clean_content = hook_source.read_text(encoding="utf-8")
        original_dirty_check = cm._dirty_tracked_hook_sources

        def dirty_after_check(repo_root: pathlib.Path) -> list[str]:
            dirty = original_dirty_check(repo_root)
            hook_source.write_text(
                "#!/bin/sh\nprintf dirty-after-check\n",
                encoding="utf-8",
            )
            hook_source.chmod(0o755)
            return dirty

        with mock.patch.dict(os.environ, GIT_ENV, clear=False):
            with mock.patch.object(cm, "_dirty_tracked_hook_sources", dirty_after_check):
                with self.assertRaisesRegex(
                    cm.CleanMergedError,
                    "tracked hook source\\(s\\) have local changes",
                ):
                    cm.install_hooks(self.work, home_dir=pathlib.Path(GIT_ENV["HOME"]))

        runtime_hooks_dir = git_common_dir_compat(self.work) / "hooks"
        self.assertFalse((runtime_hooks_dir / "post-merge").exists())
        self.assertFalse(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .exists()
        )
        self.assertIn("dirty-after-check", hook_source.read_text(encoding="utf-8"))

    def test_install_hooks_records_shadow_from_snapshot_not_live_hook(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks_dir = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks_dir.mkdir(parents=True, exist_ok=True)
        runtime_hook = runtime_hooks_dir / "post-merge"
        foreign_content = "#!/bin/sh\nprintf foreign\n"
        runtime_hook.write_text(foreign_content, encoding="utf-8")
        runtime_hook.chmod(0o755)
        repo_content = (self.work / ".githooks" / "post-merge").read_text(encoding="utf-8")
        original_record = cm._record_planned_shadowed_hook

        def mutate_only_during_record(**kwargs: Any) -> None:
            hook_file = kwargs["hook_file"]
            hook_file.write_text(repo_content, encoding="utf-8")
            hook_file.chmod(0o755)
            try:
                original_record(**kwargs)
            finally:
                hook_file.write_text(foreign_content, encoding="utf-8")
                hook_file.chmod(0o755)

        with mock.patch.dict(os.environ, GIT_ENV, clear=False):
            with mock.patch.object(
                cm,
                "_record_planned_shadowed_hook",
                mutate_only_during_record,
            ):
                cm.install_hooks(self.work, home_dir=pathlib.Path(GIT_ENV["HOME"]))

        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        shadow_entries = manifest["shadowed_hooks"]["post-merge"]
        self.assertEqual(len(shadow_entries), 1)
        backup = pathlib.Path(shadow_entries[0]["source_path"])
        self.assertEqual(backup.read_text(encoding="utf-8"), foreign_content)
        self.assertEqual(shadow_entries[0]["source_sha256"], file_sha256(backup))

    def test_git_common_dir_does_not_require_path_format_flag(self) -> None:
        source = (REPO_ROOT / "scripts" / "clean_merged_artifacts.py").read_text(
            encoding="utf-8")
        self.assertNotIn("--path-format=absolute", source)

    def test_no_preview_cleanup_fallback_delete_path(self) -> None:
        source = (REPO_ROOT / "scripts" / "clean_merged_artifacts.py").read_text(
            encoding="utf-8")
        self.assertNotIn('"update-ref", "--stdin"', source)

    def test_missing_origin_owner_is_config_error(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8").replace('origin_owner = "t"\n', ""),
            encoding="utf-8",
        )

        with self.assertRaisesRegex(cm.ConfigError, "origin_owner"):
            cm.load_config(self.work)

    def test_unknown_runtime_key_is_config_error(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8").replace(
                'origin_owner = "t"', 'origin_owner = "t"\nhook_detach = false'),
            encoding="utf-8",
        )

        with self.assertRaisesRegex(cm.ConfigError, "hook_detach"):
            cm.load_config(self.work)

    def test_trunk_resolution_uses_configured_ref_only(self) -> None:
        _run(["git", "checkout", "-b", "operator"], cwd=self.work)
        _run(["git", "branch", "-D", "main"], cwd=self.work)
        _run(["git", "update-ref", "-d", "refs/remotes/origin/main"], cwd=self.work)
        _run(["git", "branch", "master"], cwd=self.work)

        self.assertIsNone(cm.resolve_trunk_sha(self.work, "main", "origin"))

    def test_runtime_quantities_load_from_config(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8")
            .replace("gh_limit = 100", "gh_limit = 37")
            .replace("archive_timeout_s = 120", "archive_timeout_s = 12")
            .replace("archive_verify_timeout_s = 30", "archive_verify_timeout_s = 3")
            .replace("heartbeat_stale_days = 7", "heartbeat_stale_days = 2")
            .replace("rotated_log_retention_days = 30", "rotated_log_retention_days = 3")
            .replace("report_error_max_chars = 200", "report_error_max_chars = 44")
            .replace(
                'lane_r_log_path = "<git-common-dir>/clean-merged.lane-r.log"',
                'lane_r_log_path = "<git-common-dir>/custom-lane-r.log"',
            ),
            encoding="utf-8",
        )

        config = cm.load_config(self.work)

        self.assertEqual(config.lane_r.gh_limit, 37)
        self.assertEqual(config.lane_w.archive_timeout_s, 12)
        self.assertEqual(config.lane_w.archive_verify_timeout_s, 3)
        self.assertEqual(config.logging.rotated_log_retention_days, 3)
        self.assertEqual(config.logging.report_error_max_chars, 44)
        self.assertEqual(config.logging.heartbeat_stale_days, 2)
        self.assertEqual(config.logging.lane_r_log_path, "<git-common-dir>/custom-lane-r.log")

    def test_cli_print_remote_name_uses_config(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8").replace(
                'remote_name = "origin"', 'remote_name = "upstream"'),
            encoding="utf-8",
        )

        proc = run_clean_proc(self.work, "--print-remote-name")

        self.assertEqual(proc.returncode, 0)
        self.assertEqual(proc.stdout.strip(), "upstream")

    def test_cli_print_remote_name_fails_loud_without_config(self) -> None:
        (self.work / "config" / "clean-merged.toml").unlink()

        proc = run_clean_proc(self.work, "--print-remote-name")

        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual(proc.stdout, "")
        self.assertIn("config/clean-merged.toml", proc.stderr)

    def test_setup_source_uses_configured_remote_name(self) -> None:
        source = (REPO_ROOT / "justfile").read_text(encoding="utf-8")

        self.assertIn("--print-remote-name", source)
        self.assertIn("remote.${clean_merged_remote}.prune", source)
        self.assertNotIn("remote.origin.prune", source)

    def test_setup_uses_untracked_git_common_runtime_hook_directory(self) -> None:
        self._write_clean_merged_hook_sources(self.work)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(
            _run(["git", "config", "--get", "core.hooksPath"], cwd=self.work)
            .stdout.strip(),
            str(git_common_dir_compat(self.work) / "hooks"),
        )

    def test_install_hooks_preserves_existing_local_hooks_when_moving_to_runtime_dir(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        local_hook = source_hooks / "commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf local-hook\n", encoding="utf-8")
        local_hook.chmod(0o755)
        chained_hook = source_hooks / "post-rewrite.pre-entire"
        chained_hook.write_text("#!/bin/sh\nprintf chained-hook\n", encoding="utf-8")
        chained_hook.chmod(0o755)
        _run(["git", "config", "core.hooksPath", ".githooks"], cwd=self.work)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        common_dir = git_common_dir_compat(self.work)
        runtime_hooks = common_dir / "hooks"
        self.assertEqual(
            _run(["git", "config", "--get", "core.hooksPath"], cwd=self.work).stdout.strip(),
            str(runtime_hooks),
        )
        for hook in ("post-merge", "post-checkout", "post-rewrite"):
            self.assertEqual(
                (runtime_hooks / hook).read_text(encoding="utf-8"),
                (source_hooks / hook).read_text(encoding="utf-8"),
            )
            self.assertTrue(os.access(runtime_hooks / hook, os.X_OK))
        self.assertEqual(
            (runtime_hooks / "commit-msg").read_text(encoding="utf-8"),
            local_hook.read_text(encoding="utf-8"),
        )
        self.assertFalse((runtime_hooks / "post-rewrite.pre-entire").exists())

    def test_install_hooks_syncs_new_source_local_hook_after_runtime_path_is_active(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        local_hook = source_hooks / "prepare-commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf source-local-hook\n", encoding="utf-8")
        local_hook.chmod(0o755)
        _run(["git", "add", ".githooks/prepare-commit-msg"], cwd=self.work)
        _run(["git", "commit", "-m", "track prepare-commit-msg hook"], cwd=self.work)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "prepare-commit-msg"
        self.assertEqual(
            runtime_hook.read_text(encoding="utf-8"),
            local_hook.read_text(encoding="utf-8"),
        )
        self.assertTrue(os.access(runtime_hook, os.X_OK))

    def test_install_hooks_removes_repo_source_hook_after_tracked_source_removed(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        local_hook = source_hooks / "prepare-commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf source-local-hook\n", encoding="utf-8")
        local_hook.chmod(0o755)
        _run(["git", "add", ".githooks/prepare-commit-msg"], cwd=self.work)
        _run(["git", "commit", "-m", "track prepare-commit-msg hook"], cwd=self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "prepare-commit-msg"
        self.assertTrue(runtime_hook.exists())
        _run(["git", "rm", ".githooks/prepare-commit-msg"], cwd=self.work)
        _run(["git", "commit", "-m", "remove prepare-commit-msg hook"], cwd=self.work)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertFalse(runtime_hook.exists())
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertNotIn("prepare-commit-msg", manifest["hooks"])

    def test_install_hooks_does_not_sync_untracked_githooks_file_after_runtime_active(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        local_hook = source_hooks / "prepare-commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf untracked-source-hook\n", encoding="utf-8")
        local_hook.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "prepare-commit-msg"
        self.assertFalse(runtime_hook.exists())

    def test_install_hooks_ignores_tracked_non_hook_githooks_file(self) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        non_hook = source_hooks / "README"
        non_hook.write_text("not a git hook\n", encoding="utf-8")
        _run(["git", "add", ".githooks/README"], cwd=self.work)
        _run(["git", "commit", "-m", "track non hook file"], cwd=self.work)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_non_hook = git_common_dir_compat(self.work) / "hooks" / "README"
        self.assertFalse(runtime_non_hook.exists())
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertNotIn("README", manifest["hooks"])

    def test_install_hooks_does_not_sync_tracked_nested_githooks_file(self) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        nested_dir = source_hooks / "support"
        nested_dir.mkdir()
        nested_file = nested_dir / "helper"
        nested_file.write_text("#!/bin/sh\nprintf nested-helper\n", encoding="utf-8")
        nested_file.chmod(0o755)
        _run(["git", "add", ".githooks/support/helper"], cwd=self.work)
        _run(["git", "commit", "-m", "track nested hook support file"], cwd=self.work)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_helper = git_common_dir_compat(self.work) / "hooks" / "helper"
        self.assertFalse(runtime_helper.exists())

    def test_install_hooks_preserves_global_hooks_without_same_name_false_collision(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks"
        global_hooks.mkdir()
        global_post_rewrite = global_hooks / "post-rewrite"
        global_post_rewrite.write_text(
            "#!/bin/sh\nprintf global-post-rewrite\n",
            encoding="utf-8",
        )
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text(
            "#!/bin/sh\nprintf global-commit-msg\n",
            encoding="utf-8",
        )
        for hook in (global_post_rewrite, global_commit_msg):
            hook.chmod(0o755)
        global_config = self.tmp / "global.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(
            self.work,
            "--install-hooks",
            env={"GIT_CONFIG_GLOBAL": str(global_config)},
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        source_hooks = self.work / ".githooks"
        self.assertEqual(
            (runtime_hooks / "post-rewrite").read_text(encoding="utf-8"),
            (source_hooks / "post-rewrite").read_text(encoding="utf-8"),
        )
        self.assertEqual(
            (runtime_hooks / "commit-msg").read_text(encoding="utf-8"),
            global_commit_msg.read_text(encoding="utf-8"),
        )
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertEqual(manifest["hooks"]["commit-msg"]["source_scope"], "global")

    def test_install_hooks_ignores_non_hook_files_in_active_hooks_dir(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-non-hooks"
        global_hooks.mkdir()
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text("#!/bin/sh\nprintf global-commit-msg\n",
                                     encoding="utf-8")
        global_commit_msg.chmod(0o755)
        non_hook = global_hooks / "not-a-hook"
        non_hook.write_text("#!/bin/sh\nprintf not-a-hook\n", encoding="utf-8")
        non_hook.chmod(0o755)
        global_config = self.tmp / "global-non-hooks.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(
            self.work,
            "--install-hooks",
            env={"GIT_CONFIG_GLOBAL": str(global_config)},
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        self.assertTrue((runtime_hooks / "commit-msg").is_file())
        self.assertFalse((runtime_hooks / "not-a-hook").exists())
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertNotIn("not-a-hook", manifest["hooks"])

    def test_install_hooks_refreshes_adopted_global_hook_from_manifest(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-refresh"
        global_hooks.mkdir()
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_commit_msg.chmod(0o755)
        global_config = self.tmp / "global-refresh.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        global_commit_msg.write_text("#!/bin/sh\nprintf global-v2\n", encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "commit-msg"
        self.assertEqual(
            runtime_hook.read_text(encoding="utf-8"),
            global_commit_msg.read_text(encoding="utf-8"),
        )

    def test_install_hooks_tracks_global_hook_path_move(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks_v1 = self.tmp / "global-hooks-move-v1"
        global_hooks_v1.mkdir()
        global_commit_msg_v1 = global_hooks_v1 / "commit-msg"
        global_commit_msg_v1.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_commit_msg_v1.chmod(0o755)
        global_config = self.tmp / "global-move.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks_v1}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        global_hooks_v2 = self.tmp / "global-hooks-move-v2"
        global_hooks_v2.mkdir()
        global_commit_msg_v2 = global_hooks_v2 / "commit-msg"
        global_commit_msg_v2.write_text("#!/bin/sh\nprintf global-v2\n", encoding="utf-8")
        global_commit_msg_v2.chmod(0o755)
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks_v2}\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "commit-msg"
        self.assertEqual(
            runtime_hook.read_text(encoding="utf-8"),
            global_commit_msg_v2.read_text(encoding="utf-8"),
        )
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertEqual(manifest["hooks"]["commit-msg"]["source_path"], str(global_commit_msg_v2))

    def test_install_hooks_adopts_new_global_hook_added_after_runtime_active(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-live"
        global_hooks.mkdir()
        global_pre_push = global_hooks / "pre-push"
        global_pre_push.write_text("#!/bin/sh\nprintf global-pre-push\n", encoding="utf-8")
        global_pre_push.chmod(0o755)
        global_config = self.tmp / "global-live.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        global_pre_commit = global_hooks / "pre-commit"
        global_pre_commit.write_text("#!/bin/sh\nprintf global-pre-commit\n", encoding="utf-8")
        global_pre_commit.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "pre-commit"
        self.assertEqual(
            runtime_hook.read_text(encoding="utf-8"),
            global_pre_commit.read_text(encoding="utf-8"),
        )
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertEqual(manifest["hooks"]["pre-commit"]["source_path"], str(global_pre_commit))

    def test_install_hooks_tracks_local_hook_path_move(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        legacy_hooks = self.work / ".legacy-hooks"
        legacy_hooks.mkdir()
        legacy_commit_msg = legacy_hooks / "commit-msg"
        legacy_commit_msg.write_text("#!/bin/sh\nprintf legacy-local\n", encoding="utf-8")
        legacy_commit_msg.chmod(0o755)
        _run(["git", "config", "core.hooksPath", ".legacy-hooks"], cwd=self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        new_hooks = self.work / ".new-hooks"
        new_hooks.mkdir()
        new_commit_msg = new_hooks / "commit-msg"
        new_commit_msg.write_text("#!/bin/sh\nprintf new-local\n", encoding="utf-8")
        new_commit_msg.chmod(0o755)
        _run(["git", "config", "core.hooksPath", ".new-hooks"], cwd=self.work)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "commit-msg"
        self.assertEqual(
            runtime_hook.read_text(encoding="utf-8"),
            new_commit_msg.read_text(encoding="utf-8"),
        )
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertEqual(
            pathlib.Path(manifest["hooks"]["commit-msg"]["source_path"]).resolve(),
            new_commit_msg.resolve(),
        )

    def test_install_hooks_tracks_system_hook_path_move(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        system_hooks_v1 = self.tmp / "system-hooks-move-v1"
        system_hooks_v1.mkdir()
        system_commit_msg_v1 = system_hooks_v1 / "commit-msg"
        system_commit_msg_v1.write_text("#!/bin/sh\nprintf system-v1\n", encoding="utf-8")
        system_commit_msg_v1.chmod(0o755)
        system_config = self.tmp / "system-move.gitconfig"
        system_config.write_text(
            f"[core]\n\thooksPath = {system_hooks_v1}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_SYSTEM": str(system_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        system_hooks_v2 = self.tmp / "system-hooks-move-v2"
        system_hooks_v2.mkdir()
        system_commit_msg_v2 = system_hooks_v2 / "commit-msg"
        system_commit_msg_v2.write_text("#!/bin/sh\nprintf system-v2\n", encoding="utf-8")
        system_commit_msg_v2.chmod(0o755)
        system_config.write_text(
            f"[core]\n\thooksPath = {system_hooks_v2}\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "commit-msg"
        self.assertEqual(
            runtime_hook.read_text(encoding="utf-8"),
            system_commit_msg_v2.read_text(encoding="utf-8"),
        )
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertEqual(manifest["hooks"]["commit-msg"]["source_scope"], "system")
        self.assertEqual(manifest["hooks"]["commit-msg"]["source_path"], str(system_commit_msg_v2))

    def test_install_hooks_refuses_untrackable_command_scope_hooks_path(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        command_hooks = self.tmp / "command-hooks"
        command_hooks.mkdir()
        command_commit_msg = command_hooks / "commit-msg"
        command_commit_msg.write_text("#!/bin/sh\nprintf command-hook\n", encoding="utf-8")
        command_commit_msg.chmod(0o755)

        proc = run_clean_proc(
            self.work,
            "--install-hooks",
            env={
                "GIT_CONFIG_COUNT": "1",
                "GIT_CONFIG_KEY_0": "core.hooksPath",
                "GIT_CONFIG_VALUE_0": str(command_hooks),
            },
        )

        self.assertEqual(proc.returncode, 1)
        self.assertIn("unsupported command config scope", proc.stderr)

    def test_doctor_reports_untrackable_command_scope_without_setup_loop(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        command_hooks = self.tmp / "command-hooks-doctor"
        command_hooks.mkdir()
        command_commit_msg = command_hooks / "commit-msg"
        command_commit_msg.write_text("#!/bin/sh\nprintf command-hook\n", encoding="utf-8")
        command_commit_msg.chmod(0o755)

        proc = run_clean_proc(
            self.work,
            "--doctor",
            env={
                "GIT_CONFIG_COUNT": "1",
                "GIT_CONFIG_KEY_0": "core.hooksPath",
                "GIT_CONFIG_VALUE_0": str(command_hooks),
            },
        )

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("unsupported command config scope", proc.stdout)
        self.assertIn("remove or convert unsupported core.hooksPath config", proc.stdout)
        self.assertNotIn(
            "core.hooksPath is not git-common hooks directory (run `just setup`)",
            proc.stdout,
        )

    def test_install_hooks_removes_global_hook_when_global_path_unset(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-unset"
        global_hooks.mkdir()
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_commit_msg.chmod(0o755)
        global_config = self.tmp / "global-unset.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "commit-msg"
        self.assertTrue(runtime_hook.exists())
        global_config.write_text("", encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertFalse(runtime_hook.exists())
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertNotIn("commit-msg", manifest["hooks"])

    def test_install_hooks_removes_adopted_hook_when_manifest_source_disappears(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-remove"
        global_hooks.mkdir()
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_commit_msg.chmod(0o755)
        global_config = self.tmp / "global-remove.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "commit-msg"
        self.assertTrue(runtime_hook.exists())
        global_commit_msg.unlink()

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertFalse(runtime_hook.exists())
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertNotIn("commit-msg", manifest["hooks"])

    def test_doctor_reports_adopted_global_hook_source_drift(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-doctor"
        global_hooks.mkdir()
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_commit_msg.chmod(0o755)
        global_config = self.tmp / "global-doctor.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        global_commit_msg.write_text("#!/bin/sh\nprintf global-v2\n", encoding="utf-8")

        proc = run_clean_proc(self.work, "--doctor", env=env)

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("hook commit-msg source changed", proc.stdout)

    def test_doctor_reports_adopted_hook_runtime_drift_with_recovery(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-runtime-drift"
        global_hooks.mkdir()
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_commit_msg.chmod(0o755)
        global_config = self.tmp / "global-runtime-drift.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "commit-msg"
        runtime_hook.write_text("#!/bin/sh\nprintf tampered-runtime\n", encoding="utf-8")
        runtime_hook.chmod(0o755)

        setup = run_clean_proc(self.work, "--install-hooks", env=env)
        proc = run_clean_proc(self.work, "--doctor", env=env)

        self.assertEqual(setup.returncode, 1)
        self.assertIn("refusing to adopt modified runtime hook commit-msg", setup.stderr)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn(
            f"hook commit-msg runtime is outside allowed state; "
            f"remove {runtime_hook} and run `just setup`",
            proc.stdout,
        )

    def test_doctor_reports_invalid_manifest_recovery_before_hook_recovery(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest_path.write_text("{", encoding="utf-8")

        proc = run_clean_proc(self.work, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("hook manifest unreadable", proc.stdout)
        self.assertIn(
            f"repair or remove hook manifest {manifest_path} and run `just setup`",
            proc.stdout,
        )
        self.assertNotIn("runtime is outside allowed state", proc.stdout)

    def test_install_hooks_accepts_legacy_manifest_without_optional_containers(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest_path.write_text(json.dumps({"version": 1}), encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertNotIn("Traceback", proc.stderr)
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        self.assertIsInstance(manifest["hooks"], dict)
        self.assertIsInstance(manifest["shadowed_hooks"], dict)
        self.assertIsInstance(manifest["source_dirs"], list)

    def test_manifest_null_container_reports_recovery_without_traceback(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest_path.write_text(
            json.dumps({"version": 1, "hooks": None}),
            encoding="utf-8",
        )

        setup = run_clean_proc(self.work, "--install-hooks")
        doctor = run_clean_proc(self.work, "--doctor")

        self.assertEqual(setup.returncode, 1)
        self.assertIn("hooks must be object", setup.stderr)
        self.assertNotIn("Traceback", setup.stderr)
        self.assertNotEqual(doctor.returncode, 0)
        self.assertIn("hooks must be object", doctor.stdout)
        self.assertIn(
            f"repair or remove hook manifest {manifest_path} and run `just setup`",
            doctor.stdout,
        )
        self.assertNotIn("Traceback", doctor.stderr)

    def test_install_hooks_refuses_invalid_manifest_hook_entry(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["hooks"]["commit-msg"] = "not an object"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("hooks.commit-msg must be object", proc.stderr)

    def test_install_hooks_refuses_legacy_effective_manifest_scope(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-effective-manifest"
        global_hooks.mkdir()
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text("#!/bin/sh\nprintf global\n", encoding="utf-8")
        global_commit_msg.chmod(0o755)
        global_config = self.tmp / "global-effective-manifest.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["hooks"]["commit-msg"]["source_scope"] = "effective"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 1)
        self.assertIn("unsupported source_scope effective", proc.stderr)

    def test_install_hooks_refuses_non_hook_manifest_key(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["hooks"]["not-a-hook"] = {
            "source_kind": "active-hook",
            "source_scope": "default",
            "source_path": str(git_common_dir_compat(self.work) / "hooks" / "not-a-hook"),
            "source_sha256": "0" * 64,
            "runtime_sha256": "0" * 64,
        }
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("hooks.not-a-hook must be known Git hook name", proc.stderr)

    def test_install_hooks_refuses_invalid_shadowed_manifest_entry(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-invalid-shadow"
        global_hooks.mkdir()
        global_post_rewrite = global_hooks / "post-rewrite"
        global_post_rewrite.write_text("#!/bin/sh\nprintf shadow\n", encoding="utf-8")
        global_post_rewrite.chmod(0o755)
        global_config = self.tmp / "global-invalid-shadow.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["shadowed_hooks"]["post-rewrite"][0]["source_scope"] = "effective"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 1)
        self.assertIn(
            "shadowed_hooks.post-rewrite[0] unsupported source_scope effective",
            proc.stderr,
        )

    def test_install_hooks_refuses_invalid_manifest_source_dir_entry(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["source_dirs"] = [{"source_scope": "effective", "source_path": ".old-hooks"}]
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("source_dirs[0] unsupported source_scope effective", proc.stderr)

    def test_doctor_on_config_error_reports_manifest_hook_source_drift(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-doctor-config-error"
        global_hooks.mkdir()
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_commit_msg.chmod(0o755)
        global_config = self.tmp / "global-doctor-config-error.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        global_commit_msg.write_text("#!/bin/sh\nprintf global-v2\n", encoding="utf-8")
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8") + "\nunknown_key = true\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(self.work, "--doctor", env=env)

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("CONFIG ERROR", proc.stdout)
        self.assertIn("hook commit-msg source changed", proc.stdout)

    def test_install_hooks_records_shadowed_same_name_active_hook(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-shadow"
        global_hooks.mkdir()
        global_post_rewrite = global_hooks / "post-rewrite"
        global_post_rewrite.write_text("#!/bin/sh\nprintf global-post-rewrite\n",
                                       encoding="utf-8")
        global_post_rewrite.chmod(0o755)
        global_config = self.tmp / "global-shadow.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(
            self.work,
            "--install-hooks",
            env={"GIT_CONFIG_GLOBAL": str(global_config)},
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        shadowed = manifest["shadowed_hooks"]["post-rewrite"][0]
        self.assertEqual(shadowed["source_scope"], "global")
        self.assertEqual(shadowed["source_path"], str(global_post_rewrite))

    def test_install_hooks_preserves_shadowed_hook_after_runtime_path_is_active(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-shadow-preserve"
        global_hooks.mkdir()
        global_post_rewrite = global_hooks / "post-rewrite"
        global_post_rewrite.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_post_rewrite.chmod(0o755)
        global_config = self.tmp / "global-shadow-preserve.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 0, proc.stderr)
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        shadowed = manifest["shadowed_hooks"]["post-rewrite"][0]
        self.assertEqual(shadowed["source_path"], str(global_post_rewrite))

    def test_install_hooks_refreshes_shadowed_hook_source_from_manifest(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-shadow-refresh"
        global_hooks.mkdir()
        global_post_rewrite = global_hooks / "post-rewrite"
        global_post_rewrite.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_post_rewrite.chmod(0o755)
        global_config = self.tmp / "global-shadow-refresh.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        global_post_rewrite.write_text("#!/bin/sh\nprintf global-v2\n", encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks", env=env)

        self.assertEqual(proc.returncode, 0, proc.stderr)
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        shadowed = manifest["shadowed_hooks"]["post-rewrite"][0]
        self.assertEqual(shadowed["source_sha256"], file_sha256(global_post_rewrite))

    def test_doctor_reports_shadowed_same_name_source_drift(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-shadow-doctor"
        global_hooks.mkdir()
        global_post_rewrite = global_hooks / "post-rewrite"
        global_post_rewrite.write_text("#!/bin/sh\nprintf global-v1\n", encoding="utf-8")
        global_post_rewrite.chmod(0o755)
        global_config = self.tmp / "global-shadow-doctor.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )
        env = {"GIT_CONFIG_GLOBAL": str(global_config)}
        self.assertEqual(run_clean_proc(self.work, "--install-hooks", env=env).returncode, 0)
        global_post_rewrite.write_text("#!/bin/sh\nprintf global-v2\n", encoding="utf-8")

        proc = run_clean_proc(self.work, "--doctor", env=env)

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("shadowed hook post-rewrite source changed", proc.stdout)

    def test_install_hooks_adopts_default_runtime_hook_without_copy(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        local_hook = runtime_hooks / "commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf default-commit-msg\n", encoding="utf-8")
        local_hook.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(
            local_hook.read_text(encoding="utf-8"),
            "#!/bin/sh\nprintf default-commit-msg\n",
        )
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertEqual(manifest["hooks"]["commit-msg"]["source_scope"], "default")
        self.assertEqual(manifest["hooks"]["commit-msg"]["runtime_sha256"], file_sha256(local_hook))

    def test_install_hooks_shadow_records_default_runtime_same_name_hook_collision(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        colliding_hook = runtime_hooks / "post-merge"
        colliding_hook.write_text("#!/bin/sh\nprintf default-local-post-merge\n", encoding="utf-8")
        colliding_hook.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        source_hook = self.work / ".githooks" / "post-merge"
        self.assertEqual(
            colliding_hook.read_text(encoding="utf-8"),
            source_hook.read_text(encoding="utf-8"),
        )
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        shadowed = manifest["shadowed_hooks"]["post-merge"][0]
        self.assertEqual(shadowed["source_scope"], "default")
        shadowed_path = pathlib.Path(shadowed["source_path"])
        self.assertTrue(shadowed_path.is_file())
        self.assertEqual(
            shadowed_path.read_text(encoding="utf-8"),
            "#!/bin/sh\nprintf default-local-post-merge\n",
        )

    def test_install_hooks_repeated_default_runtime_adopted_hook_is_idempotent(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        local_hook = runtime_hooks / "commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf default-commit-msg\n", encoding="utf-8")
        local_hook.chmod(0o755)

        first = run_clean_proc(self.work, "--install-hooks")
        second = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertNotIn("SameFileError", second.stderr)
        self.assertEqual(
            local_hook.read_text(encoding="utf-8"),
            "#!/bin/sh\nprintf default-commit-msg\n",
        )

    def test_install_hooks_refuses_modified_default_runtime_hook_after_path_moves(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        local_hook = runtime_hooks / "commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf default-commit-msg\n", encoding="utf-8")
        local_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        local_hook.write_text("#!/bin/sh\nprintf modified-runtime\n", encoding="utf-8")
        local_hook.chmod(0o755)
        alternate_hooks = self.work / ".alternate-hooks"
        alternate_hooks.mkdir()
        _run(["git", "config", "core.hooksPath", ".alternate-hooks"], cwd=self.work)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("refusing to adopt modified runtime hook commit-msg", proc.stderr)

    def test_install_hooks_preflights_modified_runtime_before_shadowing_collision(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        local_hook = runtime_hooks / "commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf default-commit-msg\n", encoding="utf-8")
        local_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        local_hook.write_text("#!/bin/sh\nprintf modified-runtime\n", encoding="utf-8")
        local_hook.chmod(0o755)
        repo_hook = source_hooks / "applypatch-msg"
        repo_hook.write_text("#!/bin/sh\nprintf repo-applypatch\n", encoding="utf-8")
        repo_hook.chmod(0o755)
        _run(["git", "add", ".githooks/applypatch-msg"], cwd=self.work)
        _run(["git", "commit", "-m", "track applypatch hook"], cwd=self.work)
        colliding_hook = runtime_hooks / "applypatch-msg"
        colliding_hook.write_text("#!/bin/sh\nprintf local-applypatch\n", encoding="utf-8")
        colliding_hook.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("refusing to adopt modified runtime hook commit-msg", proc.stderr)
        self.assertTrue(colliding_hook.exists())
        self.assertEqual(
            colliding_hook.read_text(encoding="utf-8"),
            "#!/bin/sh\nprintf local-applypatch\n",
        )

    def test_install_hooks_preflights_shadow_collisions_before_unlinking_any(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        for hook_name in ("applypatch-msg", "commit-msg"):
            repo_hook = source_hooks / hook_name
            repo_hook.write_text(f"#!/bin/sh\nprintf repo-{hook_name}\n", encoding="utf-8")
            repo_hook.chmod(0o755)
        _run(["git", "add", ".githooks/applypatch-msg", ".githooks/commit-msg"],
             cwd=self.work)
        _run(["git", "commit", "-m", "track extra hooks"], cwd=self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        first_collision = runtime_hooks / "applypatch-msg"
        first_collision.write_text("#!/bin/sh\nprintf local-applypatch\n", encoding="utf-8")
        first_collision.chmod(0o755)
        symlink_target = self.tmp / "commit-msg-target"
        symlink_target.write_text("#!/bin/sh\nprintf local-commit\n", encoding="utf-8")
        (runtime_hooks / "commit-msg").symlink_to(symlink_target)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("refusing to shadow symlink hook", proc.stderr)
        self.assertTrue(first_collision.exists())
        self.assertEqual(
            first_collision.read_text(encoding="utf-8"),
            "#!/bin/sh\nprintf local-applypatch\n",
        )
        self.assertFalse(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json").exists()
        )

    def test_install_hooks_preflights_runtime_collisions_with_nondefault_active_dir(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        for hook_name in ("applypatch-msg", "commit-msg"):
            repo_hook = source_hooks / hook_name
            repo_hook.write_text(f"#!/bin/sh\nprintf repo-{hook_name}\n", encoding="utf-8")
            repo_hook.chmod(0o755)
        _run(["git", "add", ".githooks/applypatch-msg", ".githooks/commit-msg"],
             cwd=self.work)
        _run(["git", "commit", "-m", "track extra hooks"], cwd=self.work)
        legacy_hooks = self.work / ".legacy-hooks"
        legacy_hooks.mkdir()
        _run(["git", "config", "core.hooksPath", ".legacy-hooks"], cwd=self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        first_collision = runtime_hooks / "applypatch-msg"
        first_collision.write_text("#!/bin/sh\nprintf local-applypatch\n", encoding="utf-8")
        first_collision.chmod(0o755)
        symlink_target = self.tmp / "commit-msg-target"
        symlink_target.write_text("#!/bin/sh\nprintf local-commit\n", encoding="utf-8")
        (runtime_hooks / "commit-msg").symlink_to(symlink_target)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("refusing to shadow symlink hook", proc.stderr)
        self.assertTrue(first_collision.exists())
        self.assertEqual(
            first_collision.read_text(encoding="utf-8"),
            "#!/bin/sh\nprintf local-applypatch\n",
        )

    def test_install_hooks_repeated_default_runtime_collision_keeps_manifest_valid(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        colliding_hook = runtime_hooks / "post-merge"
        colliding_hook.write_text("#!/bin/sh\nprintf default-local-post-merge\n", encoding="utf-8")
        colliding_hook.chmod(0o755)

        first = run_clean_proc(self.work, "--install-hooks")
        second = run_clean_proc(self.work, "--install-hooks")
        third = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(third.returncode, 0, third.stderr)
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        self.assertFalse(
            any(entry["source_scope"] == "default" for entry in manifest["source_dirs"])
        )
        self.assertTrue(manifest["shadowed_hooks"]["post-merge"])

    def test_install_hooks_refuses_modified_default_shadow_backup(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        colliding_hook = runtime_hooks / "post-merge"
        colliding_hook.write_text("#!/bin/sh\nprintf default-local-post-merge\n", encoding="utf-8")
        colliding_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        shadowed_path = pathlib.Path(manifest["shadowed_hooks"]["post-merge"][0]["source_path"])
        shadowed_path.write_text("#!/bin/sh\nprintf modified-shadow\n", encoding="utf-8")
        shadowed_path.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("shadowed hook post-merge backup changed since install", proc.stderr)

    def test_install_hooks_refuses_missing_default_shadow_backup(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        colliding_hook = runtime_hooks / "post-merge"
        colliding_hook.write_text("#!/bin/sh\nprintf default-local-post-merge\n", encoding="utf-8")
        colliding_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        pathlib.Path(manifest["shadowed_hooks"]["post-merge"][0]["source_path"]).unlink()

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("shadowed hook post-merge backup missing", proc.stderr)

    def test_doctor_reports_default_shadow_backup_drift_with_recovery(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        colliding_hook = runtime_hooks / "post-merge"
        colliding_hook.write_text("#!/bin/sh\nprintf default-local-post-merge\n", encoding="utf-8")
        colliding_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        shadowed_path = pathlib.Path(manifest["shadowed_hooks"]["post-merge"][0]["source_path"])
        shadowed_path.write_text("#!/bin/sh\nprintf modified-shadow\n", encoding="utf-8")
        shadowed_path.chmod(0o755)

        proc = run_clean_proc(self.work, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("shadowed hook post-merge backup changed since install", proc.stdout)
        self.assertIn("repair or remove hook manifest before running setup", proc.stdout)
        self.assertNotIn("shadowed hook post-merge source changed since install", proc.stdout)

    def test_install_hooks_refuses_modified_promoted_default_shadow_backup(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        repo_hook = source_hooks / "pre-push"
        repo_hook.write_text("#!/bin/sh\nprintf repo-pre-push\n", encoding="utf-8")
        repo_hook.chmod(0o755)
        _run(["git", "add", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "track pre-push hook"], cwd=self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        default_hook = runtime_hooks / "pre-push"
        default_hook.write_text("#!/bin/sh\nprintf default-pre-push\n", encoding="utf-8")
        default_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        _run(["git", "rm", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "remove pre-push hook"], cwd=self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        backup_path = pathlib.Path(manifest["hooks"]["pre-push"]["source_path"])
        backup_path.write_text("#!/bin/sh\nprintf modified-backup\n", encoding="utf-8")
        backup_path.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("shadowed hook pre-push backup changed since install", proc.stderr)
        self.assertTrue((runtime_hooks / "pre-push").exists())

    def test_install_hooks_preflights_modified_default_shadow_before_repo_hook_removal(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        repo_hook = source_hooks / "pre-push"
        repo_hook.write_text("#!/bin/sh\nprintf repo-pre-push\n", encoding="utf-8")
        repo_hook.chmod(0o755)
        _run(["git", "add", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "track pre-push hook"], cwd=self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        default_hook = runtime_hooks / "pre-push"
        default_hook.write_text("#!/bin/sh\nprintf default-pre-push\n", encoding="utf-8")
        default_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        backup_path = pathlib.Path(manifest["shadowed_hooks"]["pre-push"][0]["source_path"])
        backup_path.write_text("#!/bin/sh\nprintf modified-before-promotion\n", encoding="utf-8")
        backup_path.chmod(0o755)
        repo_runtime_content = (runtime_hooks / "pre-push").read_text(encoding="utf-8")
        _run(["git", "rm", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "remove pre-push hook"], cwd=self.work)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("shadowed hook pre-push backup changed since install", proc.stderr)
        self.assertTrue((runtime_hooks / "pre-push").exists())
        self.assertEqual((runtime_hooks / "pre-push").read_text(encoding="utf-8"),
                         repo_runtime_content)

    def test_install_hooks_preflights_default_shadow_before_repo_source_copy(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        repo_hook = source_hooks / "pre-push"
        repo_hook.write_text("#!/bin/sh\nprintf repo-pre-push\n", encoding="utf-8")
        repo_hook.chmod(0o755)
        _run(["git", "add", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "track pre-push hook"], cwd=self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        default_hook = runtime_hooks / "pre-push"
        default_hook.write_text("#!/bin/sh\nprintf default-pre-push\n", encoding="utf-8")
        default_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        backup_path = pathlib.Path(manifest["shadowed_hooks"]["pre-push"][0]["source_path"])
        post_merge_runtime = runtime_hooks / "post-merge"
        post_merge_runtime_content = post_merge_runtime.read_text(encoding="utf-8")
        (source_hooks / "post-merge").write_text(
            "#!/bin/sh\n# clean-merged-managed\nprintf updated-post-merge\n",
            encoding="utf-8",
        )
        (source_hooks / "post-merge").chmod(0o755)
        _run(["git", "rm", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "add", ".githooks/post-merge"], cwd=self.work)
        _run(["git", "commit", "-m", "update post-merge and remove pre-push"], cwd=self.work)
        backup_path.write_text("#!/bin/sh\nprintf modified-before-promotion\n", encoding="utf-8")
        backup_path.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("shadowed hook pre-push backup changed since install", proc.stderr)
        self.assertEqual(post_merge_runtime.read_text(encoding="utf-8"),
                         post_merge_runtime_content)

    def test_install_hooks_refuses_missing_promoted_default_shadow_backup(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        repo_hook = source_hooks / "pre-push"
        repo_hook.write_text("#!/bin/sh\nprintf repo-pre-push\n", encoding="utf-8")
        repo_hook.chmod(0o755)
        _run(["git", "add", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "track pre-push hook"], cwd=self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        default_hook = runtime_hooks / "pre-push"
        default_hook.write_text("#!/bin/sh\nprintf default-pre-push\n", encoding="utf-8")
        default_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        _run(["git", "rm", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "remove pre-push hook"], cwd=self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        pathlib.Path(manifest["hooks"]["pre-push"]["source_path"]).unlink()

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("shadowed hook pre-push backup missing", proc.stderr)
        self.assertTrue((runtime_hooks / "pre-push").exists())

    def test_install_hooks_preflights_missing_default_shadow_before_repo_hook_removal(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        repo_hook = source_hooks / "pre-push"
        repo_hook.write_text("#!/bin/sh\nprintf repo-pre-push\n", encoding="utf-8")
        repo_hook.chmod(0o755)
        _run(["git", "add", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "track pre-push hook"], cwd=self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        default_hook = runtime_hooks / "pre-push"
        default_hook.write_text("#!/bin/sh\nprintf default-pre-push\n", encoding="utf-8")
        default_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        pathlib.Path(manifest["shadowed_hooks"]["pre-push"][0]["source_path"]).unlink()
        repo_runtime_content = (runtime_hooks / "pre-push").read_text(encoding="utf-8")
        _run(["git", "rm", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "remove pre-push hook"], cwd=self.work)

        setup = run_clean_proc(self.work, "--install-hooks")
        doctor = run_clean_proc(self.work, "--doctor")

        self.assertEqual(setup.returncode, 1)
        self.assertIn("shadowed hook pre-push backup missing", setup.stderr)
        self.assertTrue((runtime_hooks / "pre-push").exists())
        self.assertEqual((runtime_hooks / "pre-push").read_text(encoding="utf-8"),
                         repo_runtime_content)
        self.assertIn("shadowed hook pre-push backup missing", doctor.stdout)
        self.assertNotIn("hook pre-push missing (run `just setup`)", doctor.stdout)

    def test_install_hooks_preflights_promoted_shadow_before_repo_source_copy(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        repo_hook = source_hooks / "pre-push"
        repo_hook.write_text("#!/bin/sh\nprintf repo-pre-push\n", encoding="utf-8")
        repo_hook.chmod(0o755)
        _run(["git", "add", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "track pre-push hook"], cwd=self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        default_hook = runtime_hooks / "pre-push"
        default_hook.write_text("#!/bin/sh\nprintf default-pre-push\n", encoding="utf-8")
        default_hook.chmod(0o755)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        _run(["git", "rm", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "remove pre-push hook"], cwd=self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        backup_path = pathlib.Path(manifest["hooks"]["pre-push"]["source_path"])
        post_merge_runtime = runtime_hooks / "post-merge"
        post_merge_runtime_content = post_merge_runtime.read_text(encoding="utf-8")
        (source_hooks / "post-merge").write_text(
            "#!/bin/sh\n# clean-merged-managed\nprintf updated-post-merge\n",
            encoding="utf-8",
        )
        (source_hooks / "post-merge").chmod(0o755)
        _run(["git", "add", ".githooks/post-merge"], cwd=self.work)
        _run(["git", "commit", "-m", "update post-merge hook"], cwd=self.work)
        backup_path.write_text("#!/bin/sh\nprintf modified-promoted-backup\n", encoding="utf-8")
        backup_path.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("shadowed hook pre-push backup changed since install", proc.stderr)
        self.assertEqual(post_merge_runtime.read_text(encoding="utf-8"),
                         post_merge_runtime_content)

    def test_install_hooks_shadow_records_runtime_collision_with_nondefault_active_dir(
        self,
    ) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        legacy_hooks = self.work / ".legacy-hooks"
        legacy_hooks.mkdir()
        _run(["git", "config", "core.hooksPath", ".legacy-hooks"], cwd=self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        colliding_hook = runtime_hooks / "post-merge"
        colliding_hook.write_text(
            "#!/bin/sh\nprintf stale-runtime-post-merge\n",
            encoding="utf-8",
        )
        colliding_hook.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(
            colliding_hook.read_text(encoding="utf-8"),
            (source_hooks / "post-merge").read_text(encoding="utf-8"),
        )
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        shadowed = manifest["shadowed_hooks"]["post-merge"][0]
        self.assertEqual(
            pathlib.Path(shadowed["source_path"]).read_text(encoding="utf-8"),
            "#!/bin/sh\nprintf stale-runtime-post-merge\n",
        )

    def test_install_hooks_shadow_records_marker_impostor_without_body_trust(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        impostor = runtime_hooks / "post-merge"
        impostor.write_text(
            "#!/bin/sh\n# clean-merged-managed\nprintf marker-impostor\n",
            encoding="utf-8",
        )
        impostor.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(
            impostor.read_text(encoding="utf-8"),
            (self.work / ".githooks" / "post-merge").read_text(encoding="utf-8"),
        )
        manifest = json.loads(
            (git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json")
            .read_text(encoding="utf-8")
        )
        shadowed = manifest["shadowed_hooks"]["post-merge"][0]
        self.assertEqual(shadowed["source_scope"], "default")
        self.assertEqual(
            pathlib.Path(shadowed["source_path"]).read_text(encoding="utf-8"),
            "#!/bin/sh\n# clean-merged-managed\nprintf marker-impostor\n",
        )

    def test_install_hooks_refuses_mismatched_shadow_copy(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        common_dir = git_common_dir_compat(self.work)
        runtime_hooks = common_dir / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        colliding_hook = runtime_hooks / "post-merge"
        colliding_hook.write_text("#!/bin/sh\nprintf default-local-post-merge\n", encoding="utf-8")
        colliding_hook.chmod(0o755)
        shadow_dir = common_dir / "clean-merged.shadowed-hooks"
        shadow_dir.mkdir(parents=True, exist_ok=True)
        shadow_copy = shadow_dir / f"post-merge.{file_sha256(colliding_hook)}"
        shadow_copy.write_text("#!/bin/sh\nprintf wrong-shadow\n", encoding="utf-8")
        shadow_copy.chmod(0o755)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("refusing to shadow into mismatched hook", proc.stderr)

    def test_install_hooks_refuses_symlink_hook_source(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-symlink-source"
        global_hooks.mkdir()
        target = self.tmp / "external-commit-msg"
        target.write_text("#!/bin/sh\nprintf symlink-target\n", encoding="utf-8")
        target.chmod(0o755)
        (global_hooks / "commit-msg").symlink_to(target)
        global_config = self.tmp / "global-symlink-source.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(
            self.work,
            "--install-hooks",
            env={"GIT_CONFIG_GLOBAL": str(global_config)},
        )

        self.assertEqual(proc.returncode, 1)
        self.assertIn("refusing to install symlink hook source", proc.stderr)

    def test_install_hooks_refuses_symlink_hook_destination(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        runtime_hooks.mkdir(parents=True, exist_ok=True)
        outside_target = self.tmp / "outside-post-merge-target"
        (runtime_hooks / "post-merge").symlink_to(outside_target)

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn("refusing to shadow symlink hook", proc.stderr)
        self.assertFalse(outside_target.exists())

    def test_doctor_reports_symlink_managed_hook_as_outside_allowed_state(self) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "post-merge"
        target = self.tmp / "post-merge-target"
        target.write_text(
            (source_hooks / "post-merge").read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        target.chmod(0o755)
        runtime_hook.unlink()
        runtime_hook.symlink_to(target)

        proc = run_clean_proc(self.work, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn(
            f"hook post-merge runtime is outside allowed state; "
            f"remove {runtime_hook} and run `just setup`",
            proc.stdout,
        )

    def test_doctor_reports_modified_managed_hook_recovery_without_setup_loop(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "post-merge"
        runtime_hook.write_text("#!/bin/sh\nprintf tampered\n", encoding="utf-8")
        runtime_hook.chmod(0o755)

        proc = run_clean_proc(self.work, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn(
            f"hook post-merge runtime is outside allowed state; "
            f"remove {runtime_hook} and run `just setup`",
            proc.stdout,
        )
        self.assertNotIn(
            "hook post-merge lacks installer provenance (run `just setup`)",
            proc.stdout,
        )
        self.assertNotIn(
            "hook post-merge runtime does not match tracked source (run `just setup`)",
            proc.stdout,
        )

    def test_doctor_reports_non_executable_managed_hook(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "post-merge"
        runtime_hook.chmod(0o644)

        proc = run_clean_proc(self.work, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("hook post-merge is not executable (run `just setup`)", proc.stdout)

    def test_doctor_reports_non_executable_repo_source_hook(self) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        source_hook = source_hooks / "pre-push"
        source_hook.write_text("#!/bin/sh\nprintf tracked-pre-push\n", encoding="utf-8")
        source_hook.chmod(0o755)
        _run(["git", "add", ".githooks/pre-push"], cwd=self.work)
        _run(["git", "commit", "-m", "track pre-push hook"], cwd=self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "pre-push"
        runtime_hook.chmod(0o644)

        proc = run_clean_proc(self.work, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("hook pre-push is not executable (run `just setup`)", proc.stdout)

    def test_doctor_reports_missing_managed_hook_without_remove_recovery(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "post-merge"
        runtime_hook.unlink()

        proc = run_clean_proc(self.work, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("hook post-merge missing (run `just setup`)", proc.stdout)
        self.assertNotIn(
            "hook post-merge runtime is outside allowed state; "
            f"remove {runtime_hook} and run `just setup`",
            proc.stdout,
        )

    def test_install_hooks_preserves_non_executable_external_hook_mode(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        global_hooks = self.tmp / "global-hooks-nonexec"
        global_hooks.mkdir()
        global_commit_msg = global_hooks / "commit-msg"
        global_commit_msg.write_text("#!/bin/sh\nprintf disabled\n", encoding="utf-8")
        global_commit_msg.chmod(0o644)
        global_config = self.tmp / "global-nonexec.gitconfig"
        global_config.write_text(
            f"[core]\n\thooksPath = {global_hooks}\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(
            self.work,
            "--install-hooks",
            env={"GIT_CONFIG_GLOBAL": str(global_config)},
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "commit-msg"
        self.assertFalse(os.access(runtime_hook, os.X_OK))

    def test_install_hooks_refuses_invalid_shadowed_hook_manifest(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        manifest_path = (
            git_common_dir_compat(self.work) / "clean-merged.hooks-manifest.json"
        )
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["shadowed_hooks"] = []
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("shadowed_hooks must be object", proc.stderr)

    def test_install_hooks_refuses_dirty_tracked_hook_sources(self) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        (source_hooks / "post-rewrite").write_text(
            "#!/bin/sh\n# clean-merged-managed\nprintf dirty-post-rewrite\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn(
            "tracked hook source(s) have local changes: .githooks/post-rewrite",
            proc.stderr,
        )

    def test_install_hooks_refuses_dirty_tracked_source_local_hook(self) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        local_hook = source_hooks / "commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf tracked-local-hook\n", encoding="utf-8")
        local_hook.chmod(0o755)
        _run(["git", "add", ".githooks/commit-msg"], cwd=self.work)
        _run(["git", "commit", "-m", "track commit-msg hook"], cwd=self.work)
        local_hook.write_text("#!/bin/sh\nprintf dirty-local-hook\n", encoding="utf-8")

        proc = run_clean_proc(self.work, "--install-hooks")

        self.assertEqual(proc.returncode, 1)
        self.assertIn(".githooks/commit-msg", proc.stderr)

    def test_install_hooks_resolves_home_relative_active_hooks_path(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        home = self.tmp / "home"
        active_hooks = home / "hooks"
        active_hooks.mkdir(parents=True)
        local_hook = active_hooks / "commit-msg"
        local_hook.write_text("#!/bin/sh\nprintf home-local-hook\n", encoding="utf-8")
        local_hook.chmod(0o755)
        _run(["git", "config", "core.hooksPath", "~/hooks"], cwd=self.work)

        proc = run_clean_proc(self.work, "--install-hooks", env={"HOME": str(home)})

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hook = git_common_dir_compat(self.work) / "hooks" / "commit-msg"
        self.assertEqual(
            runtime_hook.read_text(encoding="utf-8"),
            local_hook.read_text(encoding="utf-8"),
        )

    def test_install_hooks_uses_main_worktree_hook_sources_from_linked_worktree(self) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work, label="main")
        _run(["git", "branch", "feat/hook-install"], cwd=self.work)
        linked = add_worktree(self.work, "feat/hook-install", self.tmp / "linked-hook-install")
        linked_source_hooks = self._write_clean_merged_hook_sources(
            linked,
            label="linked",
            track=False,
        )

        proc = run_clean_proc(linked, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        common_dir = git_common_dir_compat(self.work)
        runtime_hook = common_dir / "hooks" / "post-merge"
        self.assertEqual(
            runtime_hook.read_text(encoding="utf-8"),
            (source_hooks / "post-merge").read_text(encoding="utf-8"),
        )
        self.assertNotEqual(
            runtime_hook.read_text(encoding="utf-8"),
            (linked_source_hooks / "post-merge").read_text(encoding="utf-8"),
        )

    def test_install_hooks_updates_linked_worktree_specific_hooks_path(
        self,
    ) -> None:
        self._write_clean_merged_hook_sources(self.work)
        _run(["git", "branch", "feat/worktree-hooks-path"], cwd=self.work)
        linked = add_worktree(
            self.work,
            "feat/worktree-hooks-path",
            self.tmp / "linked-worktree-hooks-path",
        )
        _run(["git", "config", "extensions.worktreeConfig", "true"], cwd=self.work)
        linked_hooks = linked / ".linked-hooks"
        linked_hooks.mkdir()
        linked_commit_msg = linked_hooks / "commit-msg"
        linked_commit_msg.write_text(
            "#!/bin/sh\nprintf linked-commit-msg\n",
            encoding="utf-8",
        )
        linked_commit_msg.chmod(0o755)
        _run(["git", "config", "--worktree", "core.hooksPath", ".linked-hooks"],
             cwd=linked)

        proc = run_clean_proc(linked, "--install-hooks")

        self.assertEqual(proc.returncode, 0, proc.stderr)
        runtime_hooks = git_common_dir_compat(self.work) / "hooks"
        self.assertEqual(
            _run(["git", "config", "--get", "core.hooksPath"], cwd=linked)
            .stdout.strip(),
            str(runtime_hooks),
        )
        self.assertEqual(
            _run(["git", "config", "--worktree", "--get", "core.hooksPath"],
                 cwd=linked).stdout.strip(),
            str(runtime_hooks),
        )
        self.assertEqual(
            (runtime_hooks / "commit-msg").read_text(encoding="utf-8"),
            linked_commit_msg.read_text(encoding="utf-8"),
        )

    def test_doctor_checks_invoking_linked_worktree_hooks_path(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        _run(["git", "branch", "feat/doctor-worktree-hooks-path"], cwd=self.work)
        linked = add_worktree(
            self.work,
            "feat/doctor-worktree-hooks-path",
            self.tmp / "linked-doctor-worktree-hooks-path",
        )
        _run(["git", "config", "extensions.worktreeConfig", "true"], cwd=self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        linked_hooks = linked / ".linked-hooks"
        linked_hooks.mkdir()
        _run(["git", "config", "--worktree", "core.hooksPath", ".linked-hooks"],
             cwd=linked)

        proc = run_clean_proc(linked, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("core.hooksPath is not git-common hooks directory", proc.stdout)

    def test_doctor_rejects_legacy_tracked_hooks_path(self) -> None:
        self._write_clean_merged_hook_sources(self.work)
        _run(["git", "config", "core.hooksPath", ".githooks"], cwd=self.work)

        proc = run_clean_proc(self.work, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("core.hooksPath is not git-common hooks directory", proc.stdout)

    def test_doctor_reports_stale_runtime_hooks(self) -> None:
        source_hooks = self._write_clean_merged_hook_sources(self.work)
        self.assertEqual(run_clean_proc(self.work, "--install-hooks").returncode, 0)
        (source_hooks / "post-merge").write_text(
            "#!/bin/sh\n# clean-merged-managed\nprintf updated-post-merge\n",
            encoding="utf-8",
        )

        proc = run_clean_proc(self.work, "--doctor")

        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("hook post-merge runtime does not match tracked source", proc.stdout)

    def test_setup_remote_prune_snippet_sets_configured_remote(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8").replace(
                'remote_name = "origin"', 'remote_name = "upstream"'),
            encoding="utf-8",
        )
        _run(["git", "remote", "add", "upstream", "https://example.invalid/upstream.git"],
             cwd=self.work)
        script = f"""
set -euo pipefail
clean_merged_remote="$({sys.executable} {REPO_ROOT / "scripts" / "clean_merged_artifacts.py"} --print-remote-name)"
git config "remote.${{clean_merged_remote}}.prune" true
"""

        _run(["bash", "-c", script], cwd=self.work)

        self.assertEqual(git(self.work, "config", "--get", "remote.upstream.prune").strip(), "true")
        origin_prune = _run(["git", "config", "--get", "remote.origin.prune"],
                            cwd=self.work, check=False)
        self.assertNotEqual(origin_prune.returncode, 0)

    def test_docs_do_not_hardcode_configured_heartbeat_latency(self) -> None:
        source = (REPO_ROOT / "docs" / "ops" / "clean-merged-design.md").read_text(
            encoding="utf-8")
        normalized = " ".join(source.split())
        normalized_lower = normalized.lower()

        self.assertNotIn("real detection latency is 7 days", source)
        self.assertIn("configured heartbeat stale threshold (default 7 days)", normalized)
        self.assertNotIn("fail-open (corrupt/tampered", source)
        self.assertIn("invalid or future-dated gh cache entries fail closed", normalized_lower)
        self.assertNotIn("pre-purge warning", normalized_lower)
        self.assertIn("rotated_log_retention_days", normalized)
        self.assertIn("secret-redacted", normalized_lower)
        self.assertIn("report_error_max_chars", normalized)
        self.assertIn("gh cache health", normalized_lower)
        self.assertIn("rotated-log usage", normalized_lower)
        self.assertNotIn("Design provenance", source)

    def test_clean_merged_production_comments_use_domain_language(self) -> None:
        sources = [
            REPO_ROOT / "scripts" / "clean_merged_artifacts.py",
            REPO_ROOT / ".githooks" / "post-merge",
            REPO_ROOT / ".githooks" / "post-checkout",
        ]
        combined = "\n".join(path.read_text(encoding="utf-8") for path in sources)

        self.assertNotRegex(
            combined,
            r"(?i)\bround[- ]?\d|\bself-review\b|\bGPT\b|\bKimi\b|\bGrok\b|"
            r"\bGemini\b|code-assist|RECOVERY_HOLE|Design provenance",
        )

    def test_post_rewrite_comment_uses_configured_trunk(self) -> None:
        source = (REPO_ROOT / ".githooks" / "post-rewrite").read_text(encoding="utf-8")

        self.assertNotIn("local main", source)
        self.assertIn("local configured trunk", source)
        self.assertIn("configured trunk", source)

    def test_post_rewrite_adopts_entire_without_wrapper_split(self) -> None:
        source = (REPO_ROOT / ".githooks" / "post-rewrite").read_text(encoding="utf-8")

        self.assertIn("# Entire CLI hooks", source)
        self.assertIn(
            '_entire_stdin="$(mktemp "${TMPDIR:-/tmp}/entire-post-rewrite.XXXXXX" '
            '2>/dev/null || true)"',
            source,
        )
        self.assertIn('if [ -n "$_entire_stdin" ]; then', source)
        self.assertIn('if cat 2>/dev/null > "$_entire_stdin"; then', source)
        self.assertIn(
            'entire hooks git post-rewrite "$1" 2>/dev/null < "$_entire_stdin"',
            source,
        )
        self.assertIn("clean-merged Lane H dispatch", source)
        self.assertNotIn("post-rewrite.pre-entire", source)

    def test_post_rewrite_stays_silent_when_mktemp_fails(self) -> None:
        hook, clean_marker = self._install_post_rewrite_with_clean_marker()

        fake_bin = self.tmp / "fake-bin"
        fake_bin.mkdir()
        failing_mktemp = fake_bin / "mktemp"
        failing_mktemp.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
        failing_mktemp.chmod(0o755)

        proc = _run(
            [str(hook), "amend"],
            cwd=self.work,
            env={"PATH": f"{fake_bin}{os.pathsep}{os.environ['PATH']}"},
            check=False,
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stderr, "")
        self.assertTrue(clean_marker.is_file())

    def test_post_rewrite_stays_silent_when_stdin_stage_fails(self) -> None:
        hook, clean_marker = self._install_post_rewrite_with_clean_marker()

        fake_bin = self.tmp / "fake-bin-stage-fails"
        fake_bin.mkdir()
        unusable_stdin = self.tmp / "missing-stdin-dir" / "stdin"
        marker = self.tmp / "entire-invoked"
        fake_mktemp = fake_bin / "mktemp"
        fake_mktemp.write_text(
            "#!/bin/sh\n"
            'printf "%s\\n" "$FAKE_UNUSABLE_STDIN"\n',
            encoding="utf-8",
        )
        fake_mktemp.chmod(0o755)
        fake_entire = fake_bin / "entire"
        fake_entire.write_text(
            "#!/bin/sh\n"
            'printf invoked > "$FAKE_ENTIRE_MARKER"\n',
            encoding="utf-8",
        )
        fake_entire.chmod(0o755)

        proc = _run(
            [str(hook), "amend"],
            cwd=self.work,
            env={
                "PATH": f"{fake_bin}{os.pathsep}{os.environ['PATH']}",
                "FAKE_UNUSABLE_STDIN": str(unusable_stdin),
                "FAKE_ENTIRE_MARKER": str(marker),
            },
            check=False,
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stderr, "")
        self.assertFalse(marker.exists())
        self.assertTrue(clean_marker.is_file())

    def test_post_rewrite_suppresses_entire_stderr_and_dispatches_clean_merged(self) -> None:
        """Verify Entire stderr stays silent and clean-merged still dispatches.

        The `|| true` guard is forward-defense for a future stricter shell mode; with
        the current hook's no-`set -e` semantics and explicit final `exit 0`, it is
        inert and therefore not independently testable here.
        """
        hook, clean_marker = self._install_post_rewrite_with_clean_marker()

        fake_bin = self.tmp / "fake-bin-entire-errors"
        fake_bin.mkdir()
        fake_entire = fake_bin / "entire"
        fake_entire.write_text(
            "#!/bin/sh\n"
            "echo boom >&2\n"
            "exit 1\n",
            encoding="utf-8",
        )
        fake_entire.chmod(0o755)

        proc = _run(
            [str(hook), "amend"],
            cwd=self.work,
            env={"PATH": f"{fake_bin}{os.pathsep}{os.environ['PATH']}"},
            check=False,
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stderr, "")
        self.assertTrue(clean_marker.is_file())


# ---------------------------------------------------------------------------
# gh cache (A3)


class GhCacheTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-test-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_cache_avoids_repeat_call_within_ttl(self) -> None:
        config = cm.load_config(self.work)
        call_count = {"n": 0}

        def counting_fake(
            repo_root: pathlib.Path, branch: str, timeout: float, limit: int,
            report_error_max_chars: int,
        ):
            call_count["n"] += 1
            return [], None

        tip = "0" * 40  # stable tip for repeat calls
        with mock.patch.object(cm, "gh_merged_pr_for_branch", counting_fake):
            cm.gh_merged_pr_for_branch_cached(self.work, config, "feat/x", tip)
            cm.gh_merged_pr_for_branch_cached(self.work, config, "feat/x", tip)
            cm.gh_merged_pr_for_branch_cached(self.work, config, "feat/x", tip)
        self.assertEqual(call_count["n"], 1,
                          "TTL cache must collapse repeated (branch, tip) calls into one gh query")

    def test_cache_invalidates_on_tip_change(self) -> None:
        """round-4.5 self-review / Grok P2: cache key by (branch, tip), not branch alone."""
        config = cm.load_config(self.work)
        call_count = {"n": 0}

        def counting_fake(
            repo_root: pathlib.Path, branch: str, timeout: float, limit: int,
            report_error_max_chars: int,
        ):
            call_count["n"] += 1
            return [], None

        tip_a = "a" * 40
        tip_b = "b" * 40
        with mock.patch.object(cm, "gh_merged_pr_for_branch", counting_fake):
            cm.gh_merged_pr_for_branch_cached(self.work, config, "feat/x", tip_a)
            cm.gh_merged_pr_for_branch_cached(self.work, config, "feat/x", tip_b)
        self.assertEqual(call_count["n"], 2,
                          "different tips must bypass the cache (key by branch+tip)")

    def test_entry_is_live_rejects_malformed_and_non_finite(self) -> None:
        """round-5.5 polish (Claude P3-2, GPT P2): malformed/non-finite fetched_at
        must NOT crash the cache (read or save) and must not be considered live."""
        now = 1000.0
        ttl = 300.0
        # Live entry
        self.assertTrue(cm._entry_is_live({"fetched_at": now - 100, "prs": []}, now, ttl))
        # Expired entry
        self.assertFalse(cm._entry_is_live({"fetched_at": now - 1000, "prs": []}, now, ttl))
        # Malformed: string
        self.assertFalse(cm._entry_is_live({"fetched_at": "not-a-float"}, now, ttl))
        # Malformed: missing key
        self.assertFalse(cm._entry_is_live({}, now, ttl))
        # Non-finite: inf (Claude P3-2 — float('inf') parses cleanly but age = -inf)
        self.assertFalse(cm._entry_is_live({"fetched_at": float("inf")}, now, ttl))
        # Non-finite: -inf
        self.assertFalse(cm._entry_is_live({"fetched_at": float("-inf")}, now, ttl))
        # Non-finite: nan
        self.assertFalse(cm._entry_is_live({"fetched_at": float("nan")}, now, ttl))
        # Non-dict
        self.assertFalse(cm._entry_is_live(["not", "a", "dict"], now, ttl))
        self.assertFalse(cm._entry_is_live(None, now, ttl))

    def test_entry_is_live_rejects_future_fetched_at(self) -> None:
        now = 1000.0
        ttl = 300.0

        self.assertFalse(cm._entry_is_live({"fetched_at": now + 1, "prs": []}, now, ttl))

    def test_atomic_write_text_cleans_tmp_on_crash(self) -> None:
        """round-5.5 polish (Kimi/Grok P2): if write_text or os.replace raises,
        the tmp file must be cleaned up so dot-tmp.<pid> files don't accumulate."""
        target = self.tmp / "atomic-target.txt"
        target.write_text("ORIGINAL", encoding="utf-8")
        # Patch os.replace to raise; verify tmp is removed + original preserved
        # + original exception propagates.
        tmp_files_before = list(self.tmp.glob(".atomic-target.txt.tmp.*"))
        with mock.patch("os.replace", side_effect=OSError("simulated cross-filesystem")):
            with self.assertRaises(OSError):
                cm._atomic_write_text(target, "NEW")
        # Tmp cleaned up
        tmp_files_after = list(self.tmp.glob(".atomic-target.txt.tmp.*"))
        self.assertEqual(tmp_files_after, tmp_files_before,
                         "atomic write must clean up tmp file on exception")
        # Original content preserved (write_text succeeded but replace failed)
        self.assertEqual(target.read_text(encoding="utf-8"), "ORIGINAL")

    def test_cache_corruption_fails_closed_without_refetch(self) -> None:
        config = cm.load_config(self.work)
        cache_path = cm._gh_cache_path(self.work)
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        cache_path.write_text("not json at all", encoding="utf-8")
        with mock.patch.object(cm, "gh_merged_pr_for_branch") as gh:
            prs, err = cm.gh_merged_pr_for_branch_cached(
                self.work, config, "feat/x", "0" * 40)
        self.assertIsNone(prs)
        self.assertIn("cache", err or "")
        gh.assert_not_called()

    def test_live_cache_entry_with_malformed_prs_fails_closed_without_refetch(self) -> None:
        config = cm.load_config(self.work)
        branch = "feat/malformed-prs"
        tip = "1" * 40
        cache_path = cm._gh_cache_path(self.work)
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        cache_path.write_text(json.dumps({
            f"{branch}@{tip[:12]}": {
                "fetched_at": time.time(),
                "prs": {"not": "a list"},
                "error": None,
            }
        }), encoding="utf-8")
        with mock.patch.object(cm, "gh_merged_pr_for_branch") as gh:
            prs, err = cm.gh_merged_pr_for_branch_cached(self.work, config, branch, tip)

        self.assertIsNone(prs)
        self.assertIn("invalid cache entry", err or "")
        gh.assert_not_called()

    def test_future_cache_entry_fails_closed_without_refetch(self) -> None:
        config = cm.load_config(self.work)
        branch = "feat/future-cache"
        tip = "3" * 40
        cache_path = cm._gh_cache_path(self.work)
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        cache_path.write_text(json.dumps({
            f"{branch}@{tip[:12]}": {
                "fetched_at": time.time() + 3600,
                "prs": [],
            }
        }), encoding="utf-8")
        with mock.patch.object(cm, "gh_merged_pr_for_branch") as gh:
            prs, err = cm.gh_merged_pr_for_branch_cached(self.work, config, branch, tip)

        self.assertIsNone(prs)
        self.assertIn("invalid cache entry", err or "")
        gh.assert_not_called()

    def test_cache_save_prunes_invalid_entries(self) -> None:
        config = cm.load_config(self.work)
        cache_path = cm._gh_cache_path(self.work)
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        now = time.time()
        cache = {
            "bad@aaaaaaaaaaaa": {
                "fetched_at": now,
                "prs": {"not": "a list"},
            },
            "good@bbbbbbbbbbbb": {
                "fetched_at": now,
                "prs": [],
            },
        }
        cache_path.write_text(json.dumps(cache), encoding="utf-8")

        cm._save_gh_cache(cache_path, cache, config.lane_r.cache_ttl_s)

        stored = json.loads(cache_path.read_text(encoding="utf-8"))
        self.assertNotIn("bad@aaaaaaaaaaaa", stored)
        self.assertIn("good@bbbbbbbbbbbb", stored)

    def test_doctor_reports_invalid_gh_cache(self) -> None:
        cache_path = cm._gh_cache_path(self.work)
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        cache_path.write_text("not json at all", encoding="utf-8")

        proc = run_clean_proc(self.work, "--doctor")

        output = proc.stdout + proc.stderr
        self.assertEqual(proc.returncode, 1)
        self.assertIn("gh cache", output)
        self.assertIn(str(cache_path), output)

    def test_gh_failure_is_not_persisted_to_cache(self) -> None:
        config = cm.load_config(self.work)
        branch = "feat/gh-failure"
        tip = "2" * 40

        with mock.patch.object(
            cm, "gh_merged_pr_for_branch", return_value=(None, "permission denied")
        ):
            prs, err = cm.gh_merged_pr_for_branch_cached(self.work, config, branch, tip)

        self.assertIsNone(prs)
        self.assertEqual(err, "permission denied")
        cache_path = cm._gh_cache_path(self.work)
        if cache_path.exists():
            stored = json.loads(cache_path.read_text(encoding="utf-8"))
            self.assertNotIn(f"{branch}@{tip[:12]}", stored)


# ---------------------------------------------------------------------------
# Hook end-to-end tests (A4)


class HookEndToEndTests(unittest.TestCase):
    """Install the actual .githooks/* into a temp repo and verify each hook fires."""

    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-hook-"))
        self.remote = self.tmp / "remote.git"
        _run(["git", "init", "--bare", "-b", "main", str(self.remote)], cwd=self.tmp)
        self.work = self.tmp / "work"
        _run(["git", "init", "-b", "main", str(self.work)], cwd=self.tmp)
        _run(["git", "remote", "add", "origin", str(self.remote)], cwd=self.work)
        # Drop config + script + hooks
        make_config(self.work)
        scripts_dir = self.work / "scripts"
        scripts_dir.mkdir()
        shutil.copy(REPO_ROOT / "scripts" / "clean_merged_artifacts.py", scripts_dir)
        hooks_dir = self.work / ".githooks"
        hooks_dir.mkdir()
        for h in ("post-merge", "post-checkout", "post-rewrite"):
            shutil.copy(REPO_ROOT / ".githooks" / h, hooks_dir)
            (hooks_dir / h).chmod(0o755)
        _run(["git", "config", "core.hooksPath", ".githooks"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "init"], cwd=self.work)
        _run(["git", "push", "-u", "origin", "main"], cwd=self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _heartbeat(self) -> pathlib.Path | None:
        common = _run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
                       cwd=self.work).stdout.strip()
        hb = pathlib.Path(common) / "clean-merged.heartbeat"
        return hb if hb.is_file() else None

    def test_hooks_exit_zero_within_three_seconds_when_script_fails(self) -> None:
        script = self.work / "scripts" / "clean_merged_artifacts.py"
        script.write_text("raise RuntimeError('synthetic clean-merged failure')\n", encoding="utf-8")
        hooks_dir = self.work / ".githooks"
        cases = (
            ("post-merge", ()),
            ("post-checkout", ("0" * 40, "1" * 40, "1")),
            ("post-rewrite", ()),
        )
        for hook_name, args in cases:
            start = time.monotonic()
            proc = subprocess.run(
                [str(hooks_dir / hook_name), *args],
                cwd=self.work,
                env={**os.environ, **GIT_ENV},
                capture_output=True,
                text=True,
                timeout=3,
            )
            elapsed = time.monotonic() - start
            self.assertEqual(proc.returncode, 0, f"{hook_name} must fail open: {proc.stderr}")
            self.assertLess(elapsed, 3, f"{hook_name} blocked for {elapsed:.3f}s")

    def _advance_remote(self) -> None:
        other = self.tmp / "other"
        _run(["git", "clone", "-q", "-b", "main", str(self.remote), str(other)], cwd=self.tmp)
        _run(["git", "commit", "--allow-empty", "-m", "remote-advance"], cwd=other)
        _run(["git", "push", "-q", "origin", "main"], cwd=other)

    def test_post_merge_fires_on_ff_pull(self) -> None:
        # eligible branch on main; advance remote; pull -> post-merge fires Lane H.
        _run(["git", "branch", "feat/eligible"], cwd=self.work)
        self._advance_remote()
        self.assertIsNone(self._heartbeat(), "precondition: no heartbeat yet")
        _run(["git", "pull", "--ff-only", "origin", "main"], cwd=self.work)
        # detached Lane R may take a moment; heartbeat is written synchronously by Lane H
        hb = self._heartbeat()
        self.assertIsNotNone(hb, "post-merge must fire Lane H which writes the heartbeat")
        # Lane H should also have deleted the eligible branch
        branches = git(self.work, "branch", "--list")
        self.assertNotIn("feat/eligible", branches,
                          "post-merge Lane H must delete ancestor-eligible branches")

    def test_post_checkout_fires_on_branch_switch(self) -> None:
        # Switching to a feature branch and back to main fires post-checkout.
        _run(["git", "checkout", "-b", "feat/x"], cwd=self.work)
        _run(["git", "checkout", "main"], cwd=self.work)
        hb = self._heartbeat()
        # post-checkout runs Lane H only when landing on trunk; verify heartbeat present.
        self.assertIsNotNone(hb, "post-checkout Lane H dispatch must write heartbeat")

    def test_post_checkout_uses_configured_trunk_branch(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8").replace(
                'trunk_branch = "main"', 'trunk_branch = "trunk"'),
            encoding="utf-8",
        )
        _run(["git", "checkout", "-b", "trunk"], cwd=self.work)
        common = _run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
                       cwd=self.work).stdout.strip()
        pathlib.Path(common, "clean-merged.heartbeat").unlink(missing_ok=True)

        _run(["git", "checkout", "-b", "feat/configured-trunk"], cwd=self.work)
        self.assertIsNone(
            self._heartbeat(),
            "post-checkout must not dispatch when landing off the configured trunk",
        )
        _run(["git", "checkout", "trunk"], cwd=self.work)

        self.assertIsNotNone(
            self._heartbeat(),
            "post-checkout must dispatch when landing on the TOML-configured trunk",
        )

    def test_post_merge_uses_configured_lane_r_log_redirect_and_setsid_fallback(self) -> None:
        source = (REPO_ROOT / ".githooks" / "post-merge").read_text(encoding="utf-8")
        self.assertIn("--redirect-output-to-lane-r-log", source)
        self.assertIn("setsid", source)
        self.assertIn('setsid python3 "$script" --reconcile', source)
        self.assertNotIn('(setsid python3 "$script"', source)
        self.assertNotIn("clean-merged.lane-r.log", source)

    def test_redirect_output_to_configured_lane_r_log(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8").replace(
                'lane_r_log_path = "<git-common-dir>/clean-merged.lane-r.log"',
                'lane_r_log_path = "<git-common-dir>/custom-lane-r.log"',
            ),
            encoding="utf-8",
        )
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip())
        log_path = common / "custom-lane-r.log"

        proc = run_clean_proc(self.work, "--doctor", "--redirect-output-to-lane-r-log")

        self.assertEqual(proc.stdout, "")
        self.assertTrue(log_path.is_file())
        self.assertIn("[clean-merged] doctor", log_path.read_text(encoding="utf-8"))

    def test_redirect_output_rotates_lane_r_log(self) -> None:
        cfg = self.work / "config" / "clean-merged.toml"
        cfg.write_text(
            cfg.read_text(encoding="utf-8")
            .replace("max_log_bytes = 1048576", "max_log_bytes = 10")
            .replace(
                'lane_r_log_path = "<git-common-dir>/clean-merged.lane-r.log"',
                'lane_r_log_path = "<git-common-dir>/custom-lane-r.log"',
            ),
            encoding="utf-8",
        )
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip())
        log_path = common / "custom-lane-r.log"
        log_path.write_text("old content that exceeds the configured cap\n", encoding="utf-8")

        proc = run_clean_proc(self.work, "--doctor", "--redirect-output-to-lane-r-log")

        self.assertEqual(proc.stdout, "")
        self.assertTrue(log_path.with_suffix(log_path.suffix + ".1").is_file())
        self.assertIn("old content", log_path.with_suffix(log_path.suffix + ".1").read_text(
            encoding="utf-8"))
        self.assertIn("[clean-merged] doctor", log_path.read_text(encoding="utf-8"))

    def test_lane_r_log_rotation_respects_existing_lock(self) -> None:
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip())
        log_path = common / "clean-merged.lane-r.log"
        log_path.write_text("old content that exceeds the configured cap\n", encoding="utf-8")
        lock_fd = cm._acquire_lock(log_path.with_suffix(log_path.suffix + ".lock"))
        self.assertIsNotNone(lock_fd)
        assert lock_fd is not None
        try:
            handle = cm._open_rotating_log(log_path, 10, rotated_retention_days=30)
            try:
                handle.write("new content\n")
            finally:
                handle.close()
        finally:
            cm._release_lock(lock_fd)

        self.assertFalse(log_path.with_suffix(log_path.suffix + ".1").exists())
        self.assertIn("new content", log_path.read_text(encoding="utf-8"))

    def test_lane_r_log_rotation_keeps_active_writer_reachable(self) -> None:
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip())
        log_path = common / "clean-merged.lane-r.log"
        log_path.write_text("seed content that exceeds the configured cap\n", encoding="utf-8")

        active = cm._open_rotating_log(log_path, 10, rotated_retention_days=30)
        try:
            active.write("active writer before later rotations\n")
            active.flush()
            second = cm._open_rotating_log(log_path, 10, rotated_retention_days=30)
            try:
                second.write("second writer content beyond cap\n")
            finally:
                second.close()
            active.write("active writer after second rotation\n")
            active.flush()
            third = cm._open_rotating_log(log_path, 10, rotated_retention_days=30)
            try:
                third.write("third writer content\n")
            finally:
                third.close()
            active.write("active writer after third rotation\n")
            active.flush()
        finally:
            active.close()

        visible = "\n".join(
            path.read_text(encoding="utf-8")
            for path in sorted(log_path.parent.glob(f"{log_path.name}*"))
            if path.is_file() and not path.name.endswith(".lock")
        )
        self.assertIn("active writer after third rotation", visible)

    def test_rotating_log_prunes_expired_rotated_segments(self) -> None:
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip())
        log_path = common / "clean-merged.lane-r.log"
        log_path.write_text("live content that exceeds the configured cap\n", encoding="utf-8")
        expired = common / "clean-merged.lane-r.log.1.111.222"
        fresh = common / "clean-merged.lane-r.log.1.333.444"
        expired.write_text("expired\n", encoding="utf-8")
        fresh.write_text("fresh\n", encoding="utf-8")
        now = time.time()
        os.utime(expired, (now - 3 * 86400, now - 3 * 86400))
        os.utime(fresh, (now, now))

        handle = cm._open_rotating_log(log_path, 10, rotated_retention_days=2)
        handle.close()

        self.assertFalse(expired.exists())
        self.assertTrue(fresh.exists())
        self.assertTrue(log_path.with_suffix(log_path.suffix + ".1").exists())

    def test_post_rewrite_fires_on_rebase_pull(self) -> None:
        # Divergent local + remote forces an actual rebase on pull -> post-rewrite fires.
        _run(["git", "commit", "--allow-empty", "-m", "local-divergent"], cwd=self.work)
        self._advance_remote()
        _run(["git", "pull", "--rebase", "origin", "main"], cwd=self.work)
        hb = self._heartbeat()
        self.assertIsNotNone(hb, "post-rewrite must fire Lane H which writes the heartbeat")

    def test_kill_switch_parity_disabled_zero_does_not_silence(self) -> None:
        """round-4.5 self-review: bash hooks used `[ -n ]` (any non-empty disables),
        Python used `_is_disabled` (empty/0/false/no/off = enabled). The mismatch
        meant CLEAN_MERGED_DISABLED=0 silenced hooks but enabled manual Python runs.
        Assert parity: DISABLED=0 does NOT silence the hook (matches Python)."""
        # eligible branch on main; advance remote
        _run(["git", "branch", "feat/parity"], cwd=self.work)
        self._advance_remote()
        # Remove any prior heartbeat so we can detect a fresh write.
        common = _run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
                       cwd=self.work).stdout.strip()
        pathlib.Path(common, "clean-merged.heartbeat").unlink(missing_ok=True)
        # Pull with DISABLED=0 in the environment.
        pull_env = {"CLEAN_MERGED_DISABLED": "0"}
        full_env = os.environ.copy()
        full_env.update(GIT_ENV)
        full_env.update(pull_env)
        subprocess.run(
            ["git", "pull", "--ff-only", "origin", "main"],
            cwd=self.work, env=full_env, capture_output=True, text=True, timeout=30,
        )
        hb = pathlib.Path(common, "clean-merged.heartbeat")
        self.assertTrue(hb.is_file(),
                        "CLEAN_MERGED_DISABLED=0 must NOT silence the hook (parity with Python)")

    def test_kill_switch_parity_disabled_one_silences(self) -> None:
        """Counterpart: CLEAN_MERGED_DISABLED=1 must silence both bash and Python."""
        _run(["git", "branch", "feat/parity-off"], cwd=self.work)
        self._advance_remote()
        common = _run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
                       cwd=self.work).stdout.strip()
        pathlib.Path(common, "clean-merged.heartbeat").unlink(missing_ok=True)
        full_env = os.environ.copy()
        full_env.update(GIT_ENV)
        full_env.update({"CLEAN_MERGED_DISABLED": "1"})
        subprocess.run(
            ["git", "pull", "--ff-only", "origin", "main"],
            cwd=self.work, env=full_env, capture_output=True, text=True, timeout=30,
        )
        hb = pathlib.Path(common, "clean-merged.heartbeat")
        self.assertFalse(hb.is_file(),
                          "CLEAN_MERGED_DISABLED=1 must silence the hook")


    def test_atomic_write_survives_interrupted_write(self) -> None:
        """round-5 P1: _atomic_write_text uses tmp + os.replace so a crash during
        the write leaves the previous content intact (never partial/empty)."""
        target = self.tmp / "target.txt"
        target.write_text("ORIGINAL", encoding="utf-8")
        # Simulate an interruption by passing text that would crash mid-write.
        # We just verify the function is "atomic" in the structural sense:
        # the tmp file is not the target file until os.replace succeeds.
        cm._atomic_write_text(target, "NEW")
        self.assertEqual(target.read_text(encoding="utf-8"), "NEW")
        # No leftover tmp files
        leftovers = [p for p in target.parent.iterdir() if ".tmp." in p.name]
        self.assertEqual(leftovers, [], "atomic write must clean up tmp files")

    def test_purge_quarantine_preserves_verified_archive(self) -> None:
        """round-5 Grok P1: cruft-purge must NOT delete a worktree_remove_ok=False
        dir if worktree.tar.gz exists and verifies. Otherwise a crash between
        worktree-remove and the manifest flip would lose the only recovery surface."""
        # Synthesize an incomplete quarantine entry with a verified archive.
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip()).resolve()
        q = common / "clean-merged-quarantine" / "stuck-dir"
        q.mkdir(parents=True)
        # Create a real tar (so tar -tzf succeeds).
        payload = self.tmp / "payload"
        payload.mkdir()
        (payload / "f.txt").write_text("recoverable", encoding="utf-8")
        subprocess.run(["tar", "-czf", str(q / "worktree.tar.gz"), "-C", str(self.tmp), "payload"],
                       check=True, capture_output=True)
        # Manifest with worktree_remove_ok=False (the stuck state).
        (q / "clean-merged.manifest.json").write_text(json.dumps({
            "branch": "feat/stuck", "worktree_remove_ok": False,
        }), encoding="utf-8")
        # Backdate mtime so it's past grace.
        old_ts = time.time() - 31 * 86400
        os.utime(q, (old_ts, old_ts))
        rc = run_clean(self.work, "--purge-quarantine", "30")
        self.assertEqual(rc, 0)
        self.assertTrue((q / "worktree.tar.gz").is_file(),
                        "verified archive must NOT be cruft-purged — it's the recovery surface")

    def test_purge_quarantine_removes_dir_when_verified_archive_gone(self) -> None:
        """round-5.5 polish-2 (GPT P2, Claude P3-1): verified_archive_at flag
        must NOT pin a dir forever if the archive was deleted externally.
        The flag records 'was valid once', not 'is valid now'. Without the
        archive_file.is_file() gate, an empty stuck dir survives indefinitely."""
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip()).resolve()
        q = common / "clean-merged-quarantine" / "stuck-flagged-empty"
        q.mkdir(parents=True)
        # Manifest claims a verified archive, but NO archive file is present
        # (simulating external deletion/corruption that removed it).
        (q / "clean-merged.manifest.json").write_text(json.dumps({
            "branch": "feat/flag-only", "worktree_remove_ok": False,
            "verified_archive_at": "2026-01-01T00:00:00+00:00",
        }), encoding="utf-8")
        old_ts = time.time() - 31 * 86400
        os.utime(q, (old_ts, old_ts))
        rc = run_clean(self.work, "--purge-quarantine", "30")
        self.assertEqual(rc, 0)
        self.assertFalse(q.exists(),
                         "empty stuck dir must be purged even with stale verified_archive_at flag")


# ---------------------------------------------------------------------------
# Round-5: active-worktree skip (Grok P1)


class ActiveWorktreeSkipTests(unittest.TestCase):
    """Lane W invoked from inside a non-main worktree must not archive that worktree."""

    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-skip-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_lane_w_does_not_clean_invoker_worktree(self) -> None:
        # Set up an eligible (ancestor-of-trunk) branch + worktree on it.
        _run(["git", "branch", "feat/invoker"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)
        wt_path = self.tmp / "wt-invoker"
        add_worktree(self.work, "feat/invoker", wt_path)
        # Run Lane W with cwd=wt_path (the invoker IS in the feature worktree).
        # _main_worktree_root(wt_path) resolves to self.work; invoke_root = wt_path;
        # Lane W's skip must protect wt_path.
        old_cwd = os.getcwd()
        os.chdir(wt_path)
        try:
            # Invoke with cwd=wt_path. cm.main() will resolve invoke_root from cwd.
            old_env = os.environ.copy()
            os.environ.update(GIT_ENV)
            try:
                rc = cm.main(["--include-worktrees", "--apply", "--quiet"])
            finally:
                os.environ.clear()
                os.environ.update(old_env)
        finally:
            # If Lane W correctly preserved wt_path, we can chdir back to it
            # then to old_cwd. If it removed wt_path, the chdir(old_cwd) below
            # would still work (absolute path) — the assertion catches the bug.
            try:
                os.chdir(old_cwd)
            except OSError:
                # Process CWD may be invalid if wt_path was removed; force it.
                os.chdir(self.tmp)
        self.assertEqual(rc, 0)
        self.assertTrue(wt_path.exists(),
                        "Lane W must NOT archive the worktree the operator is standing in")
        self.assertIn("feat/invoker", git(self.work, "branch", "--list"))

    def test_detached_invoker_does_not_skip_other_detached_worktrees(self) -> None:
        """round-5.5 polish (Claude P3-1, GPT P2, Kimi P2): the dropped
        `wt.branch is not None` guard meant an invoker on detached HEAD
        (cur=None) caused EVERY detached worktree to match None==None → skip.
        Restored guard + invoke_root path check ensures only the invoker's
        own worktree is skipped; other eligible detached worktrees still
        get cleaned.

        round-soundness update: detached-HEAD worktrees are now REFUSED by
        default (GPT P0 RECOVERY_HOLE). This test now passes
        --allow-detached-removal to exercise the underlying skip logic."""
        # Two ancestor-of-trunk detached worktrees: wt-other (eligible, should
        # be removed) and wt-invoker (the operator's CWD, must survive).
        # Create commits on trunk so detached HEADs land at ancestor-of-trunk.
        _run(["git", "commit", "--allow-empty", "-m", "c1"], cwd=self.work)
        c1_sha = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "commit", "--allow-empty", "-m", "c2"], cwd=self.work)
        # Push so origin/main advances past c1; otherwise resolve_trunk_sha
        # returns stale origin/main (at init) and c1 looks non-ancestor.
        _run(["git", "push", "-q", "origin", "main"], cwd=self.work)
        # wt-other at c1 (ancestor of current main); detached.
        wt_other = self.tmp / "wt-other"
        _run(["git", "worktree", "add", "--detach", str(wt_other), c1_sha], cwd=self.work)
        # wt-invoker at c1 too (detached); the operator's CWD.
        wt_invoker = self.tmp / "wt-invoker-detached"
        _run(["git", "worktree", "add", "--detach", str(wt_invoker), c1_sha], cwd=self.work)
        old_cwd = os.getcwd()
        os.chdir(wt_invoker)
        try:
            old_env = os.environ.copy()
            os.environ.update(GIT_ENV)
            try:
                # --allow-detached-removal: explicitly override the post-soundness
                # default refuse so the underlying skip logic is exercised.
                rc = cm.main(["--include-worktrees", "--apply",
                              "--allow-detached-removal", "--quiet"])
            finally:
                os.environ.clear()
                os.environ.update(old_env)
        finally:
            try:
                os.chdir(old_cwd)
            except OSError:
                os.chdir(self.tmp)
        self.assertEqual(rc, 0)
        # wt-other (the eligible non-invoker detached worktree) MUST be removed.
        self.assertFalse(wt_other.exists(),
                         "Detached invoker must NOT cause other detached worktrees "
                         "to be over-skipped (round-5.5 None-guard regression)")
        # wt-invoker (the operator's CWD) MUST survive via invoke_root path check.
        self.assertTrue(wt_invoker.exists(),
                        "Invoker's detached worktree must still be protected")


# ---------------------------------------------------------------------------
# Soundness-gap regression: Lane W is OPT-IN (round-soundness Claude finding #1)
# A future refactor that accidentally hook-fires Lane W would violate the
# foundational "never do irreversible work in a hook" invariant. These tests
# pin the invariant: default mode + --reconcile MUST NOT remove worktrees.


class LaneWOptInInvariantTests(unittest.TestCase):
    """Pin the architectural invariant that Lane W is opt-in.

    Without these, flipping --include-worktrees's default to True (or adding
    run_lane_w() to the default else-branch) would silently violate the
    'no irreversible work in a hook' contract and 43 tests would stay green.
    """

    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-optin-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _setup_eligible_worktree_bound_branch(self) -> pathlib.Path:
        _run(["git", "branch", "feat/wb"], cwd=self.work)
        _run(["git", "commit", "--allow-empty", "-m", "trunk-ahead"], cwd=self.work)
        _run(["git", "push", "-q", "origin", "main"], cwd=self.work)
        wt_path = self.tmp / "wt-wb"
        add_worktree(self.work, "feat/wb", wt_path)
        return wt_path

    def test_default_mode_does_not_run_lane_w(self) -> None:
        """--apply without --include-worktrees must NOT touch worktrees."""
        wt_path = self._setup_eligible_worktree_bound_branch()
        # Plain --apply (no --include-worktrees, no --lane w)
        rc = run_clean(self.work, "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertTrue(wt_path.exists(),
                        "default mode MUST NOT remove worktrees (Lane W is opt-in)")
        self.assertIn("feat/wb", git(self.work, "branch", "--list"))
        # No quarantine created
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip()).resolve()
        self.assertFalse((common / "clean-merged-quarantine").exists(),
                         "default mode MUST NOT create quarantine")

    def test_reconcile_apply_does_not_run_lane_w(self) -> None:
        """--reconcile --apply (the hook's invocation pattern) must NOT touch worktrees."""
        wt_path = self._setup_eligible_worktree_bound_branch()
        rc = run_clean(self.work, "--reconcile", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertTrue(wt_path.exists(),
                        "--reconcile MUST NOT remove worktrees (Lane W is opt-in)")
        self.assertIn("feat/wb", git(self.work, "branch", "--list"))
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip()).resolve()
        self.assertFalse((common / "clean-merged-quarantine").exists(),
                         "--reconcile MUST NOT create quarantine")

    def test_detached_worktree_refused_without_flag(self) -> None:
        """Soundness GPT P0 / Kimi RECOVERY_HOLE: detached-HEAD worktree with
        reflog-only commits must be REFUSED by default. The archive doesn't
        capture the orphaned commit; the worktree's reflog dies with the admin
        entry on `git worktree remove`; commit becomes unreachable."""
        # Set up: trunk with two commits, detached worktree at the older one.
        _run(["git", "commit", "--allow-empty", "-m", "c1"], cwd=self.work)
        c1_sha = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "commit", "--allow-empty", "-m", "c2"], cwd=self.work)
        _run(["git", "push", "-q", "origin", "main"], cwd=self.work)
        wt_path = self.tmp / "wt-detached"
        _run(["git", "worktree", "add", "--detach", str(wt_path), c1_sha], cwd=self.work)
        # Default Lane W invocation: detached must be refused.
        rc = run_clean(self.work, "--include-worktrees", "--apply", "--quiet")
        self.assertEqual(rc, 0)
        self.assertTrue(wt_path.exists(),
                        "detached-HEAD worktree MUST be refused by default "
                        "(reflog-only commits are not preserved by the archive)")
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip()).resolve()
        self.assertFalse((common / "clean-merged-quarantine").exists(),
                         "no quarantine should be created for refused detached worktree")

    def test_detached_worktree_removed_with_allow_flag(self) -> None:
        """Soundness-fix review (Kimi P2 #3): --allow-detached-removal actually
        allows removal. Without this test, the override flag could be silently
        no-op and the suite would stay green."""
        _run(["git", "commit", "--allow-empty", "-m", "c1"], cwd=self.work)
        c1_sha = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "commit", "--allow-empty", "-m", "c2"], cwd=self.work)
        _run(["git", "push", "-q", "origin", "main"], cwd=self.work)
        wt_path = self.tmp / "wt-detached-allowed"
        _run(["git", "worktree", "add", "--detach", str(wt_path), c1_sha], cwd=self.work)
        rc = run_clean(self.work, "--include-worktrees", "--apply",
                       "--allow-detached-removal", "--quiet")
        self.assertEqual(rc, 0)
        self.assertFalse(wt_path.exists(),
                         "--allow-detached-removal must actually permit removal "
                         "of an eligible detached-HEAD worktree")
        common = pathlib.Path(_run(["git", "rev-parse", "--path-format=absolute",
                                     "--git-common-dir"], cwd=self.work).stdout.strip()).resolve()
        self.assertTrue((common / "clean-merged-quarantine").is_dir(),
                        "quarantine must be created when --allow-detached-removal proceeds")

    def test_non_git_dir_exits_gracefully(self) -> None:
        """Soundness-fix review (gemini P2 / learnings #9 degraded-state): invoked
        from a directory that is NOT a git repo, the tool must exit 0 (hook-safe —
        never break the git op) for the normal lanes and exit non-zero with a clean
        diagnostic, NOT a Python traceback, for --doctor. Guards the non-git-dir
        (_resolve_repo_root) arm of the widened `except CleanMergedError`; the
        pre-fix code raised an uncaught CleanMergedError (traceback, rc!=0). The
        config-missing arm is guarded by
        test_git_repo_without_config_exits_gracefully."""
        nongit = pathlib.Path(tempfile.mkdtemp(prefix="cm-nongit-"))
        self.addCleanup(shutil.rmtree, nongit, ignore_errors=True)
        env = os.environ.copy()
        env.update(GIT_ENV)
        # Precondition (self-review P2): `git rev-parse` walks UP the tree, so if
        # $TMPDIR is nested inside a git checkout, mkdtemp() lands in a repo and the
        # tool would resolve the PARENT repo — the test would pass for the wrong
        # reason and --apply could mutate that repo. Require a genuinely repo-less
        # dir; skip (don't silently pass) otherwise.
        if subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=nongit,
                          env=env, capture_output=True, text=True).returncode == 0:
            self.skipTest(f"{nongit} is inside a git repo (set TMPDIR outside any "
                          "checkout); cannot exercise the non-git-dir path here")
        script = str(REPO_ROOT / "scripts" / "clean_merged_artifacts.py")

        def run_in_nongit(*args: str) -> subprocess.CompletedProcess[str]:
            return subprocess.run([sys.executable, script, *args], cwd=nongit,
                                  env=env, capture_output=True, text=True, timeout=60)

        # Normal hook invocations must never break the git op -> exit 0, no crash.
        for args in (("--quiet",), ("--lane", "h", "--apply", "--quiet")):
            proc = run_in_nongit(*args)
            self.assertEqual(proc.returncode, 0,
                             f"{args} from a non-git dir must exit 0; stderr={proc.stderr!r}")
            self.assertNotIn("Traceback", proc.stdout + proc.stderr,
                             f"{args} from a non-git dir must not crash with a traceback")
        # --doctor surfaces a diagnostic (rc=1) but still must not crash.
        doc = run_in_nongit("--doctor")
        self.assertEqual(doc.returncode, 1,
                         f"--doctor from a non-git dir should report a problem (rc=1); stderr={doc.stderr!r}")
        self.assertNotIn("Traceback", doc.stdout + doc.stderr,
                         "--doctor from a non-git dir must not crash with a traceback")

    def test_git_repo_without_config_exits_gracefully(self) -> None:
        """Self-review P2 (the config-missing arm of the gemini fix): a real git
        repo with NO config/clean-merged.toml makes load_config raise ConfigError
        (a subclass of CleanMergedError). The tool must exit 0 for the normal
        lanes and rc=1 with a clean diagnostic (no traceback) for --doctor — the
        same contract as the non-git arm. Pins the second behavior of the widened
        `except CleanMergedError` so re-narrowing/re-raising it can't silently
        regress."""
        nocfg_root = pathlib.Path(tempfile.mkdtemp(prefix="cm-nocfg-"))
        self.addCleanup(shutil.rmtree, nocfg_root, ignore_errors=True)
        repo = make_repo(nocfg_root)  # real git repo, but deliberately NO make_config()
        env = os.environ.copy()
        env.update(GIT_ENV)
        script = str(REPO_ROOT / "scripts" / "clean_merged_artifacts.py")

        def run_in_repo(*args: str) -> subprocess.CompletedProcess[str]:
            return subprocess.run([sys.executable, script, *args], cwd=repo,
                                  env=env, capture_output=True, text=True, timeout=60)

        # Normal hook invocations must never break the git op -> exit 0, no crash.
        for args in (("--quiet",), ("--lane", "h", "--apply", "--quiet")):
            proc = run_in_repo(*args)
            self.assertEqual(proc.returncode, 0,
                             f"{args} in a config-less repo must exit 0; stderr={proc.stderr!r}")
            self.assertNotIn("Traceback", proc.stdout + proc.stderr,
                             f"{args} in a config-less repo must not crash with a traceback")
        # --doctor surfaces the config diagnostic (rc=1) but still must not crash.
        doc = run_in_repo("--doctor")
        self.assertEqual(doc.returncode, 1,
                         f"--doctor in a config-less repo should report a problem (rc=1); stderr={doc.stderr!r}")
        self.assertNotIn("Traceback", doc.stdout + doc.stderr,
                         "--doctor in a config-less repo must not crash with a traceback")

    def test_missing_tomllib_exits_gracefully(self) -> None:
        """A Python without stdlib tomllib must not die before doctor diagnostics."""
        shadow_dir = self.tmp / "shadow-tomllib"
        shadow_dir.mkdir()
        script = shadow_dir / "clean_merged_artifacts.py"
        shutil.copy(REPO_ROOT / "scripts" / "clean_merged_artifacts.py", script)
        (shadow_dir / "tomllib.py").write_text(
            "raise ModuleNotFoundError(\"No module named 'tomllib'\")\n",
            encoding="utf-8",
        )
        env = os.environ.copy()
        env.update(GIT_ENV)

        normal = subprocess.run(
            [sys.executable, str(script), "--lane", "h", "--apply", "--quiet"],
            cwd=self.work, env=env, capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(normal.returncode, 0)
        self.assertNotIn("Traceback", normal.stdout + normal.stderr)

        doctor = subprocess.run(
            [sys.executable, str(script), "--doctor"],
            cwd=self.work, env=env, capture_output=True, text=True, timeout=60,
        )
        output = doctor.stdout + doctor.stderr
        self.assertEqual(doctor.returncode, 1)
        self.assertNotIn("Traceback", output)
        self.assertIn("tomllib=no", output)
        self.assertIn("Python 3.11+", output)

    def test_detached_refusal_uses_distinct_action_and_no_sentinel_leak(self) -> None:
        """Soundness-fix review (Kimi P2 #1 / Claude): a refused detached-HEAD
        worktree must be recorded with the distinct action 'refused-detached-head'
        (so audit/doctor queries can find it) and must NOT leak the internal
        sentinel into the operator-facing reason. Asserts the structured record —
        a stdout-only check would miss a sentinel leak, and the pre-existing refuse
        test asserts only survival + no-quarantine, not the action label."""
        _run(["git", "commit", "--allow-empty", "-m", "c1"], cwd=self.work)
        c1_sha = _run(["git", "rev-parse", "HEAD"], cwd=self.work).stdout.strip()
        _run(["git", "commit", "--allow-empty", "-m", "c2"], cwd=self.work)
        _run(["git", "push", "-q", "origin", "main"], cwd=self.work)
        wt_path = self.tmp / "wt-detached-label"
        _run(["git", "worktree", "add", "--detach", str(wt_path), c1_sha], cwd=self.work)
        old_env = os.environ.copy()
        os.environ.update(GIT_ENV)
        try:
            config = cm.load_config(self.work)
            # apply=False: the eligibility refusal is recorded before any mutation,
            # so a dry run exercises the label mapping without removing anything.
            records = cm.run_lane_w(
                self.work, config, apply=False, keep=set(), quiet=True,
                discard_ignored=False, remove_nested=False, discard_hidden=False)
        finally:
            os.environ.clear()
            os.environ.update(old_env)
        refused = [r for r in records if r["action"] == "refused-detached-head"]
        self.assertEqual(len(refused), 1,
                         f"detached refusal must use the distinct 'refused-detached-head' "
                         f"action label; records={records}")
        for r in records:
            self.assertNotIn("__REFUSED_DETACHED", r.get("reason", ""),
                             f"internal sentinel must not leak into the reason: {r}")


class LaneTTargetDirReaperTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="cm-lane-t-"))
        self.work = make_repo(self.tmp)
        make_config(self.work)

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _linked_worktree(self, branch: str = "feat/target-dir") -> pathlib.Path:
        _run(["git", "branch", branch], cwd=self.work)
        return add_worktree(self.work, branch, self.tmp / branch.replace("/", "-"))

    def _fake_ps_env(self, body: str = "exit 0\n", *, lsof_body: str | None = None) -> dict[str, str]:
        bin_dir = self.tmp / "bin"
        bin_dir.mkdir(exist_ok=True)
        ps = bin_dir / "ps"
        ps.write_text(f"#!/usr/bin/env bash\n{body}", encoding="utf-8")
        ps.chmod(0o755)
        if lsof_body is not None:
            lsof = bin_dir / "lsof"
            lsof.write_text(f"#!/usr/bin/env bash\n{lsof_body}", encoding="utf-8")
            lsof.chmod(0o755)
        return {"PATH": f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}"}

    @staticmethod
    def _age_subtree(path: pathlib.Path, *, days: int) -> None:
        old_time = time.time() - (days * 24 * 60 * 60)
        for child in sorted(path.rglob("*"), reverse=True):
            os.utime(child, (old_time, old_time))
        os.utime(path, (old_time, old_time))

    def test_target_dir_reaper_dry_run_then_apply_reaps_idle_candidate(self) -> None:
        append_lane_t_config(self.work)
        wt = self._linked_worktree()
        target = wt / "target"
        artifact = target / "debug" / "artifact"
        artifact.parent.mkdir(parents=True)
        artifact.write_text("old", encoding="utf-8")
        self._age_subtree(target, days=8)
        env = self._fake_ps_env()

        dry = run_clean_proc(self.work, "--include-target-dirs", env=env)

        self.assertEqual(dry.returncode, 0, dry.stderr)
        self.assertTrue(target.exists(), "dry-run must not remove target dirs")
        self.assertIn("target-dir-reap-candidate", dry.stdout)

        applied = run_clean_proc(self.work, "--include-target-dirs", "--apply", "--quiet", env=env)

        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertFalse(target.exists(), "apply must remove eligible idle target dir")

    def test_target_dir_reaper_honors_keep_branch_in_single_lane_mode(self) -> None:
        append_lane_t_config(self.work)
        branch = "feat/keep-target-dir"
        wt = self._linked_worktree(branch)
        target = wt / "target"
        artifact = target / "debug" / "artifact"
        artifact.parent.mkdir(parents=True)
        artifact.write_text("old", encoding="utf-8")
        self._age_subtree(target, days=8)
        env = self._fake_ps_env()

        applied = run_clean_proc(
            self.work, "--lane", "t", "--apply", "--quiet", "--keep", branch, env=env,
        )

        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertTrue(target.exists(), "Lane T must honor --keep for worktree target dirs")

    def test_target_dir_reaper_spares_recent_inner_mtime(self) -> None:
        append_lane_t_config(self.work)
        wt = self._linked_worktree("feat/recent-target-dir")
        target = wt / "target"
        artifact = target / "debug" / "fresh-artifact"
        artifact.parent.mkdir(parents=True)
        artifact.write_text("fresh", encoding="utf-8")
        old_time = time.time() - (30 * 24 * 60 * 60)
        os.utime(target, (old_time, old_time))
        env = self._fake_ps_env()

        applied = run_clean_proc(self.work, "--include-target-dirs", "--apply", "--quiet", env=env)

        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertTrue(target.exists(), "fresh inner file must keep a stale top-level target dir")

    def test_target_dir_reaper_refuses_active_process_reference(self) -> None:
        append_lane_t_config(self.work, active_process_patterns=("cargo",))
        wt = self._linked_worktree("feat/active-target-dir")
        target = wt / "target"
        artifact = target / "debug" / "artifact"
        artifact.parent.mkdir(parents=True)
        artifact.write_text("old", encoding="utf-8")
        self._age_subtree(target, days=8)
        proc_dir = self.tmp / "proc" / "123"
        proc_dir.mkdir(parents=True)
        (proc_dir / "cwd").symlink_to(target)
        env = self._fake_ps_env("printf '123 cargo build\\n'\n")
        env["CLEAN_MERGED_PROCESS_CWD_BASE"] = str(self.tmp / "proc")

        applied = run_clean_proc(self.work, "--include-target-dirs", "--apply", env=env)

        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertTrue(target.exists(), "active target dir must not be removed")
        self.assertIn("target-dir-refused-active-process", applied.stdout)

    def test_target_dir_reaper_refuses_renamed_rust_build_process(self) -> None:
        append_lane_t_config(self.work, active_process_patterns=("cargo",))
        wt = self._linked_worktree("feat/renamed-rust-target-dir")
        target = wt / "target"
        artifact = target / "debug" / "artifact"
        artifact.parent.mkdir(parents=True)
        artifact.write_text("old", encoding="utf-8")
        self._age_subtree(target, days=8)
        proc_dir = self.tmp / "proc" / "123"
        proc_dir.mkdir(parents=True)
        (proc_dir / "cwd").symlink_to(target)
        env = self._fake_ps_env("printf '123 build-wrapper build --manifest-path Cargo.toml\\n'\n")
        env["CLEAN_MERGED_PROCESS_CWD_BASE"] = str(self.tmp / "proc")

        applied = run_clean_proc(self.work, "--include-target-dirs", "--apply", env=env)

        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertTrue(target.exists(), "renamed Rust build process must block target dir removal")
        self.assertIn("target-dir-refused-active-process", applied.stdout)

    def test_target_dir_process_timeouts_come_from_config(self) -> None:
        append_lane_t_config(
            self.work,
            active_process_patterns=("cargo",),
            process_list_timeout_s=12,
            cwd_visibility_timeout_s=3,
        )
        config = cm.load_config(self.work)
        self.assertIsNotNone(config.lane_t)
        target = self.work / "target"
        target.mkdir()
        calls: list[float] = []

        def fake_run(cmd: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
            calls.append(kwargs["timeout"])
            if cmd[0] == "ps":
                return subprocess.CompletedProcess(cmd, 0, "123 cargo build\n", "")
            if cmd[0] == "lsof":
                return subprocess.CompletedProcess(cmd, 0, f"n{target}\n", "")
            raise AssertionError(cmd)

        old_base = os.environ.get("CLEAN_MERGED_PROCESS_CWD_BASE")
        os.environ["CLEAN_MERGED_PROCESS_CWD_BASE"] = str(self.tmp / "missing-proc")
        try:
            with mock.patch.object(cm.shutil, "which", return_value="/usr/bin/lsof"):
                with mock.patch.object(cm.subprocess, "run", fake_run):
                    active, error = cm.active_target_dir_processes(self.work, target, config.lane_t)
        finally:
            if old_base is None:
                os.environ.pop("CLEAN_MERGED_PROCESS_CWD_BASE", None)
            else:
                os.environ["CLEAN_MERGED_PROCESS_CWD_BASE"] = old_base

        self.assertIsNone(error)
        self.assertEqual(calls, [12, 3])
        self.assertEqual(len(active), 1)

    def test_target_dir_reaper_refuses_if_tree_changes_before_delete(self) -> None:
        append_lane_t_config(self.work)
        wt = self._linked_worktree("feat/racy-target-dir")
        target = wt / "target"
        artifact = target / "debug" / "artifact"
        artifact.parent.mkdir(parents=True)
        artifact.write_text("old", encoding="utf-8")
        self._age_subtree(target, days=8)
        env = self._fake_ps_env(f"touch {target / 'debug' / 'new-artifact'}\nexit 0\n")

        applied = run_clean_proc(self.work, "--include-target-dirs", "--apply", env=env)

        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertTrue(target.exists(), "target dir changed before delete must not be removed")
        self.assertIn("target-dir-refused-changed-before-delete", applied.stdout)

    def test_target_dir_reaper_uses_lsof_when_proc_cwd_is_unavailable(self) -> None:
        append_lane_t_config(self.work, active_process_patterns=("cargo",))
        wt = self._linked_worktree("feat/lsof-target-dir")
        target = wt / "target"
        artifact = target / "debug" / "artifact"
        artifact.parent.mkdir(parents=True)
        artifact.write_text("old", encoding="utf-8")
        self._age_subtree(target, days=8)
        env = self._fake_ps_env(
            "printf '123 cargo build\\n'\n",
            lsof_body=f"printf 'p123\\nfcwd\\nn{target}\\n'\n",
        )
        env["CLEAN_MERGED_PROCESS_CWD_BASE"] = str(self.tmp / "missing-proc")

        applied = run_clean_proc(self.work, "--include-target-dirs", "--apply", env=env)

        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertTrue(target.exists(), "lsof-visible active target dir must not be removed")
        self.assertIn("target-dir-refused-active-process", applied.stdout)

    def test_target_dir_reaper_refuses_when_matching_process_cwd_visibility_fails(self) -> None:
        append_lane_t_config(self.work, active_process_patterns=("cargo",))
        wt = self._linked_worktree("feat/no-process-visibility")
        target = wt / "target"
        artifact = target / "debug" / "artifact"
        artifact.parent.mkdir(parents=True)
        artifact.write_text("old", encoding="utf-8")
        self._age_subtree(target, days=8)
        env = self._fake_ps_env(
            "printf '123 cargo build\\n'\n",
            lsof_body="printf 'permission denied\\n' >&2\nexit 1\n",
        )
        env["CLEAN_MERGED_PROCESS_CWD_BASE"] = str(self.tmp / "missing-proc")

        applied = run_clean_proc(self.work, "--include-target-dirs", "--apply", env=env)

        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertTrue(target.exists(), "unknown process cwd must fail closed")
        self.assertIn("target-dir-refused-process-visibility", applied.stdout)

    def test_target_dir_reaper_without_config_table_is_noop(self) -> None:
        wt = self._linked_worktree("feat/no-lane-t")
        target = wt / "target"
        (target / "debug").mkdir(parents=True)
        self._age_subtree(target, days=30)
        env = self._fake_ps_env()

        applied = run_clean_proc(self.work, "--include-target-dirs", "--apply", "--quiet", env=env)

        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertTrue(target.exists(), "missing lane_t config table must no-op")


class CleanMergedDegradedStateTests(unittest.TestCase):
    def test_help_works_from_bare_non_git_directory(self) -> None:
        nongit = pathlib.Path(tempfile.mkdtemp(prefix="cm-help-nongit-"))
        self.addCleanup(shutil.rmtree, nongit, ignore_errors=True)
        env = os.environ.copy()
        env.update(GIT_ENV)
        if subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=nongit,
                          env=env, capture_output=True, text=True).returncode == 0:
            self.skipTest(f"{nongit} is inside a git repo")

        proc = subprocess.run(
            [sys.executable, str(REPO_ROOT / "scripts" / "clean_merged_artifacts.py"), "--help"],
            cwd=nongit, env=env, capture_output=True, text=True, timeout=3,
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("usage:", proc.stdout)


if __name__ == "__main__":
    import lane_governor
    lane_governor.acquire()
    unittest.main(verbosity=2)
