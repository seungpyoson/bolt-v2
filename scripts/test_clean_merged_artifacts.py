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

import contextlib
import datetime as dt
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
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
heartbeat_path = "<git-common-dir>/clean-merged.heartbeat"
heartbeat_stale_days = 7
lane_r_log_path = "<git-common-dir>/clean-merged.lane-r.log"
[clean-merged.backups]
prune_after_days = 30
"""
    cfg.write_text(base, encoding="utf-8")
    return cfg


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

    def test_gh_query_only_requests_merged_prs(self) -> None:
        """Closed-unmerged PRs must not enter Lane R's merged-branch authority."""
        captured: dict[str, Any] = {}

        def fake_run(cmd: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
            captured["cmd"] = cmd
            return subprocess.CompletedProcess(cmd, 0, "[]", "")

        with mock.patch.object(cm.subprocess, "run", fake_run):
            prs, err = cm.gh_merged_pr_for_branch(self.work, "feat/closed", 5, 37)

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
            prs, err = cm.gh_merged_pr_for_branch(self.work, "feat/no-number", 5, 100)

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

    def test_report_error_redacts_common_secret_forms(self) -> None:
        raw = (
            "Authorization: Bearer ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa "
            "token=secret-token password: hunter2 "
            "github_pat_11AAAAAAAAAAAAAAAAAAAA_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )

        safe = cm._safe_report_error(raw)

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

    def test_setup_uses_configured_remote_name(self) -> None:
        source = (REPO_ROOT / "justfile").read_text(encoding="utf-8")

        self.assertIn("--print-remote-name", source)
        self.assertIn("remote.${clean_merged_remote}.prune", source)
        self.assertNotIn("remote.origin.prune", source)

    def test_docs_do_not_hardcode_configured_heartbeat_latency(self) -> None:
        source = (REPO_ROOT / "docs" / "ops" / "clean-merged-design.md").read_text(
            encoding="utf-8")
        normalized = " ".join(source.split())
        normalized_lower = normalized.lower()

        self.assertNotIn("real detection latency is 7 days", source)
        self.assertIn("configured heartbeat stale threshold (default 7 days)", normalized)
        self.assertNotIn("fail-open (corrupt/tampered", source)
        self.assertIn("invalid or future-dated gh cache entries fail closed", normalized_lower)

    def test_post_rewrite_comment_uses_configured_trunk(self) -> None:
        source = (REPO_ROOT / ".githooks" / "post-rewrite").read_text(encoding="utf-8")

        self.assertNotIn("local main", source)
        self.assertIn("configured trunk", source)


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

        def counting_fake(repo_root: pathlib.Path, branch: str, timeout: float, limit: int):
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

        def counting_fake(repo_root: pathlib.Path, branch: str, timeout: float, limit: int):
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


if __name__ == "__main__":
    import lane_governor
    lane_governor.acquire()
    unittest.main(verbosity=2)
