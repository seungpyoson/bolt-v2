#!/usr/bin/env python3
"""Auto-cleanup of merged branches and worktrees.

Three execution lanes split by trust/speed profile:

- Lane H (hook, always-on, offline, reflog-safe):
    `git merge-base --is-ancestor` + `git branch -d` for non-worktree-bound
    ancestor branches. No gh. Never bare `-D`.
- Lane R (reconcile, network-bound):
    Per-branch `gh pr list --head <B>` matched on `headRefOid == tip` AND
    `baseRefName == trunk` AND same-repo. CAS `update-ref -d`. Skips
    worktree-bound branches (those flow to Lane W).
- Lane W (worktree, explicit):
    Owns worktree-bound branches end-to-end BEFORE the ref is deleted.
    `git worktree move` to quarantine (atomic), then `git branch -D`.
    Fail-closed on ignored content, assume-unchanged/skip-worktree bits,
    nested `.git`.

See docs/ops/clean-merged-design.md for the full design and accepted risks.
Config lives in config/clean-merged.toml (single source of truth).
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import fcntl
import json
import os
import pathlib
import re
import subprocess
import sys
import time
import tomllib
from typing import Any

SCRIPT_NAME = "clean-merged"
HOOK_MARKER = f"# {SCRIPT_NAME}-managed"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHORT_SHA_RE = re.compile(r"^[0-9a-f]{7,40}$")
LOCK_FILE = "clean-merged.lock"


class CleanMergedError(RuntimeError):
    """Base error. Always caught at the top level so hooks never break git."""


class ConfigError(CleanMergedError):
    pass


# ---------------------------------------------------------------------------
# Config


@dataclasses.dataclass(frozen=True)
class LaneRConfig:
    gh_timeout_s: float
    cache_ttl_s: float
    hook_spawn_detached: bool


@dataclasses.dataclass(frozen=True)
class LaneWConfig:
    quarantine_dir: str
    quarantine_grace_days: int
    discard_ignored: bool
    remove_nested_repos: bool
    discard_hidden_index_bits: bool


@dataclasses.dataclass(frozen=True)
class LoggingConfig:
    audit_format: str
    audit_path: str
    max_log_bytes: int
    heartbeat_path: str


@dataclasses.dataclass(frozen=True)
class BackupsConfig:
    prune_after_days: int


@dataclasses.dataclass(frozen=True)
class Config:
    enabled: bool
    trunk_branch: str
    remote_name: str
    hook_detach: bool
    lane_r: LaneRConfig
    lane_w: LaneWConfig
    logging: LoggingConfig
    backups: BackupsConfig
    origin_owner: str | None  # resolved from remote URL, may be None offline


def _resolve_repo_root(start: pathlib.Path) -> pathlib.Path:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            cwd=start, check=True, capture_output=True, text=True,
        )
    except subprocess.CalledProcessError as exc:
        raise CleanMergedError(f"not inside a git repo: {exc.stderr.strip()}") from exc
    return pathlib.Path(out.stdout.strip())


def _main_worktree_root(repo_root: pathlib.Path) -> pathlib.Path:
    """Resolve the MAIN worktree root (where config/docs live), not a feature worktree."""
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            cwd=repo_root, check=True, capture_output=True, text=True,
        )
    except subprocess.CalledProcessError as exc:
        raise CleanMergedError(f"cannot resolve git-common-dir: {exc.stderr.strip()}") from exc
    common_dir = pathlib.Path(out.stdout.strip())
    # common-dir's parent is the main worktree root
    return common_dir.parent


def _resolve_origin_owner(repo_root: pathlib.Path, remote_name: str) -> str | None:
    """Resolve the repo owner from the remote URL for same-repo PR filtering."""
    try:
        out = subprocess.run(
            ["git", "remote", "get-url", remote_name],
            cwd=repo_root, capture_output=True, text=True,
        )
    except subprocess.CalledProcessError:
        return None
    if out.returncode != 0:
        return None
    url = out.stdout.strip()
    # SSH: git@github.com:owner/repo.git
    m = re.match(r"git@[^:]+:([^/]+)/([^/]+?)(?:\.git)?$", url)
    if m:
        return m.group(1)
    # HTTPS: https://github.com/owner/repo(.git)?
    m = re.match(r"https?://[^/]+/([^/]+)/([^/]+?)(?:\.git)?$", url)
    if m:
        return m.group(1)
    return None


def _get_nested(data: dict[str, Any], dotted: str, default: Any, required: bool = False) -> Any:
    parts = dotted.split(".")
    cur: Any = data
    for p in parts[:-1]:
        if not isinstance(cur, dict) or p not in cur:
            cur = {}
            break
        cur = cur[p]
    last = parts[-1]
    if isinstance(cur, dict) and last in cur:
        return cur[last]
    if required:
        raise ConfigError(f"missing required config: {dotted}")
    return default


def load_config(repo_root: pathlib.Path) -> Config:
    """Load config from the MAIN worktree path (not the current worktree,
    which may be on a feature branch predating config/clean-merged.toml).

    Honors TOML nesting ([clean-merged.lane_r].gh_timeout_s). Missing required
    keys fail loud; missing optional keys use documented defaults.
    """
    main_root = _main_worktree_root(repo_root)
    cfg_path = main_root / "config" / "clean-merged.toml"
    if not cfg_path.is_file():
        raise ConfigError(
            f"config not found: {cfg_path}. Run from a checkout that has "
            "config/clean-merged.toml (the main worktree)."
        )
    try:
        data = tomllib.loads(cfg_path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        raise ConfigError(f"invalid TOML in {cfg_path}: {exc}") from exc

    enabled = bool(_get_nested(data, "clean-merged.enabled", True))
    trunk_branch = str(_get_nested(data, "clean-merged.trunk_branch", "main"))
    remote_name = str(_get_nested(data, "clean-merged.remote_name", "origin"))
    hook_detach = bool(_get_nested(data, "clean-merged.hook_detach", False))

    lane_r = LaneRConfig(
        gh_timeout_s=float(_get_nested(data, "clean-merged.lane_r.gh_timeout_s", 5.0)),
        cache_ttl_s=float(_get_nested(data, "clean-merged.lane_r.cache_ttl_s", 300.0)),
        hook_spawn_detached=bool(_get_nested(data, "clean-merged.lane_r.hook_spawn_detached", True)),
    )
    lane_w = LaneWConfig(
        quarantine_dir=str(_get_nested(data, "clean-merged.lane_w.quarantine_dir",
                                        "<git-common-dir>/clean-merged-quarantine")),
        quarantine_grace_days=int(_get_nested(data, "clean-merged.lane_w.quarantine_grace_days", 30)),
        discard_ignored=bool(_get_nested(data, "clean-merged.lane_w.discard_ignored", False)),
        remove_nested_repos=bool(_get_nested(data, "clean-merged.lane_w.remove_nested_repos", False)),
        discard_hidden_index_bits=bool(
            _get_nested(data, "clean-merged.lane_w.discard_hidden_index_bits", False)),
    )
    logging_cfg = LoggingConfig(
        audit_format=str(_get_nested(data, "clean-merged.logging.audit_format", "jsonl")),
        audit_path=str(_get_nested(data, "clean-merged.logging.audit_path",
                                    "<git-common-dir>/clean-merged.log")),
        max_log_bytes=int(_get_nested(data, "clean-merged.logging.max_log_bytes", 1_048_576)),
        heartbeat_path=str(_get_nested(data, "clean-merged.logging.heartbeat_path",
                                        "<git-common-dir>/clean-merged.heartbeat")),
    )
    backups = BackupsConfig(
        prune_after_days=int(_get_nested(data, "clean-merged.backups.prune_after_days", 30)),
    )
    origin_owner = _resolve_origin_owner(repo_root, remote_name)

    return Config(
        enabled=enabled, trunk_branch=trunk_branch, remote_name=remote_name,
        hook_detach=hook_detach, lane_r=lane_r, lane_w=lane_w,
        logging=logging_cfg, backups=backups, origin_owner=origin_owner,
    )


# ---------------------------------------------------------------------------
# Git helpers


def git_common_dir(repo_root: pathlib.Path) -> pathlib.Path:
    out = subprocess.run(
        ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
        cwd=repo_root, check=True, capture_output=True, text=True,
    )
    return pathlib.Path(out.stdout.strip())


def _git(repo_root: pathlib.Path, args: list[str], *,
         check: bool = True, timeout: float | None = None,
         env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    full_env = os.environ.copy()
    if env:
        full_env.update(env)
    return subprocess.run(
        ["git", *args], cwd=repo_root, check=check, capture_output=True,
        text=True, timeout=timeout, env=full_env,
    )


@dataclasses.dataclass(frozen=True)
class Branch:
    name: str
    sha: str


@dataclasses.dataclass(frozen=True)
class Worktree:
    path: pathlib.Path
    head: str
    branch: str | None  # None = detached


def list_local_branches(repo_root: pathlib.Path) -> list[Branch]:
    out = _git(repo_root, ["for-each-ref", "--format=%(refname:short)\t%(objectname)",
                            "refs/heads/"])
    branches: list[Branch] = []
    for line in out.stdout.splitlines():
        if not line.strip():
            continue
        name, sha = line.split("\t", 1)
        branches.append(Branch(name=name.strip(), sha=sha.strip()))
    return branches


def current_branch(repo_root: pathlib.Path) -> str | None:
    out = _git(repo_root, ["symbolic-ref", "--short", "HEAD"], check=False)
    if out.returncode != 0:
        return None  # detached HEAD
    return out.stdout.strip() or None


def resolve_trunk_sha(repo_root: pathlib.Path, trunk: str, remote: str) -> str | None:
    """Resolve trunk SHA, preferring remote-tracking (fresh post-fetch)."""
    for ref in (f"refs/remotes/{remote}/{trunk}", f"refs/heads/{trunk}"):
        out = _git(repo_root, ["rev-parse", "--verify", ref], check=False)
        if out.returncode == 0:
            return out.stdout.strip()
    # dynamic fallback master <-> main
    fallback = "master" if trunk == "main" else "main"
    for ref in (f"refs/remotes/{remote}/{fallback}", f"refs/heads/{fallback}"):
        out = _git(repo_root, ["rev-parse", "--verify", ref], check=False)
        if out.returncode == 0:
            return out.stdout.strip()
    return None


def is_ancestor(repo_root: pathlib.Path, tip: str, ancestor_of: str) -> bool:
    out = _git(repo_root, ["merge-base", "--is-ancestor", tip, ancestor_of], check=False)
    return out.returncode == 0


def list_worktrees(repo_root: pathlib.Path) -> list[Worktree]:
    out = _git(repo_root, ["worktree", "list", "--porcelain"])
    worktrees: list[Worktree] = []
    wt_path: pathlib.Path | None = None
    wt_head: str = ""
    wt_branch: str | None = None
    for line in out.stdout.splitlines():
        if not line:
            if wt_path is not None:
                worktrees.append(Worktree(path=wt_path, head=wt_head, branch=wt_branch))
            wt_path, wt_head, wt_branch = None, "", None
            continue
        if line.startswith("worktree "):
            wt_path = pathlib.Path(line[len("worktree "):])
        elif line.startswith("HEAD "):
            wt_head = line[len("HEAD "):].strip()
        elif line.startswith("branch "):
            ref = line[len("branch "):].strip()
            wt_branch = ref[len("refs/heads/"):] if ref.startswith("refs/heads/") else ref
        elif line.strip() == "detached":
            wt_branch = None
    if wt_path is not None:
        worktrees.append(Worktree(path=wt_path, head=wt_head, branch=wt_branch))
    return worktrees


def worktree_bound_branches(repo_root: pathlib.Path) -> set[str]:
    """Names of branches checked out in any worktree (including main)."""
    return {wt.branch for wt in list_worktrees(repo_root) if wt.branch}


# ---------------------------------------------------------------------------
# Backup refs


def backup_ref_name(branch: str, tip: str, now_ts: int) -> str:
    safe_branch = re.sub(r"[^A-Za-z0-9._/-]", "_", branch)
    return f"refs/clean-merged/{safe_branch}-{tip[:12]}-{now_ts}"


def write_backup_ref(repo_root: pathlib.Path, branch: str, tip: str) -> str:
    ref = backup_ref_name(branch, tip, int(time.time()))
    _git(repo_root, ["update-ref", ref, tip])
    return ref


def delete_branch_with_d(repo_root: pathlib.Path, branch: str) -> tuple[bool, str]:
    """Lane H: `git branch -d`. Refuses if not merged (free safety guard)."""
    out = _git(repo_root, ["branch", "-d", branch], check=False)
    return out.returncode == 0, out.stderr.strip()


def delete_branch_with_force(repo_root: pathlib.Path, branch: str) -> tuple[bool, str]:
    """Lane W post-move: `git branch -D` after explicit eligibility verification."""
    out = _git(repo_root, ["branch", "-D", branch], check=False)
    return out.returncode == 0, out.stderr.strip()


def delete_branch_cas(repo_root: pathlib.Path, branch: str, expected_sha: str) -> tuple[bool, str]:
    """Lane R: `git update-ref -d refs/heads/<branch> <expected_sha>` (CAS)."""
    out = _git(repo_root, ["update-ref", "-d", f"refs/heads/{branch}", expected_sha], check=False)
    return out.returncode == 0, out.stderr.strip()


# ---------------------------------------------------------------------------
# Audit log + heartbeat


def _resolve_path(repo_root: pathlib.Path, raw: str) -> pathlib.Path:
    if raw.startswith("<git-common-dir>/"):
        return git_common_dir(repo_root) / raw[len("<git-common-dir>/"):]
    p = pathlib.Path(raw)
    return p if p.is_absolute() else repo_root / p


def _acquire_lock(lock_path: pathlib.Path, exclusive: bool = True) -> int | None:
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(str(lock_path), os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH | fcntl.LOCK_NB)
    except BlockingIOError:
        os.close(fd)
        return None
    return fd


def _release_lock(fd: int) -> None:
    fcntl.flock(fd, fcntl.LOCK_UN)
    os.close(fd)


def write_audit(repo_root: pathlib.Path, config: Config, record: dict[str, Any]) -> None:
    log_path = _resolve_path(repo_root, config.logging.audit_path)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = log_path.with_suffix(log_path.suffix + ".lock")
    fd = _acquire_lock(lock_path)
    if fd is None:
        # best-effort; never break the op over logging
        return
    try:
        if log_path.exists() and log_path.stat().st_size > config.logging.max_log_bytes:
            rotated = log_path.with_suffix(log_path.suffix + ".1")
            try:
                rotated.unlink(missing_ok=True)
                log_path.rename(rotated)
            except OSError:
                pass
        record_with_ts = {"ts": dt.datetime.now(dt.timezone.utc).isoformat(), **record}
        with log_path.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(record_with_ts, ensure_ascii=False, sort_keys=True) + "\n")
    finally:
        _release_lock(fd)


def write_heartbeat(repo_root: pathlib.Path, config: Config) -> None:
    path = _resolve_path(repo_root, config.logging.heartbeat_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        path.write_text(dt.datetime.now(dt.timezone.utc).isoformat(), encoding="utf-8")
    except OSError:
        pass


# ---------------------------------------------------------------------------
# Lane R: gh reconcile


_GH_ENV = {"GH_PROMPT_DISABLED": "1", "GIT_TERMINAL_PROMPT": "0", "NO_COLOR": "1"}


def gh_merged_pr_for_branch(
    repo_root: pathlib.Path, branch: str, timeout: float,
) -> tuple[list[dict[str, Any]] | None, str | None]:
    """Return (prs, error). prs=None means gh trouble (keep the branch)."""
    cmd = [
        "gh", "pr", "list", "--head", branch, "--state", "merged",
        "--json", "number,headRefOid,baseRefName,headRepositoryOwner,isCrossRepository",
        "--limit", "5",
    ]
    try:
        out = subprocess.run(
            cmd, cwd=repo_root, capture_output=True, text=True,
            timeout=timeout, env={**os.environ, **_GH_ENV},
        )
    except subprocess.TimeoutExpired:
        return None, "gh timeout"
    if out.returncode != 0:
        return None, f"gh exit {out.returncode}: {out.stderr.strip()[:200]}"
    try:
        prs = json.loads(out.stdout) if out.stdout.strip() else []
    except json.JSONDecodeError as exc:
        return None, f"gh malformed json: {exc}"
    if not isinstance(prs, list):
        return None, "gh non-list payload"
    return prs, None


def find_matching_merged_pr(
    prs: list[dict[str, Any]], tip: str, trunk: str, origin_owner: str | None,
) -> dict[str, Any] | None:
    """Return the PR whose headRefOid == tip AND baseRefName == trunk AND same-repo."""
    for pr in prs:
        head_oid = pr.get("headRefOid")
        if not isinstance(head_oid, str) or not SHA_RE.match(head_oid):
            continue
        if head_oid != tip:
            continue
        if pr.get("baseRefName") != trunk:
            continue
        if pr.get("isCrossRepository"):
            continue
        if origin_owner is not None:
            owner_obj = pr.get("headRepositoryOwner")
            owner_login = owner_obj.get("login") if isinstance(owner_obj, dict) else None
            if owner_login != origin_owner:
                continue
        return pr
    return None


# ---------------------------------------------------------------------------
# Lane W: filesystem guards


def has_hidden_index_bits(wt_path: pathlib.Path) -> list[str]:
    """Return paths with assume-unchanged/skip-worktree bits (lowercase flag in ls-files -v)."""
    out = subprocess.run(
        ["git", "-C", str(wt_path), "ls-files", "-v"],
        capture_output=True, text=True, check=True,
    )
    flagged: list[str] = []
    for line in out.stdout.splitlines():
        if not line:
            continue
        flag = line[0]
        # lowercase letters = assume-unchanged or skip-worktree bit set on a tracked file
        if flag.islower() and flag.isalpha():
            flagged.append(line[2:].strip())
    return flagged


def has_ignored_content(wt_path: pathlib.Path) -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(wt_path), "-c", "status.showUntrackedFiles=all",
         "ls-files", "--others", "--ignored", "--exclude-standard", "-z"],
        capture_output=True, text=True, check=True,
    )
    return [p for p in out.stdout.split("\0") if p]


def has_nested_git(wt_path: pathlib.Path) -> list[pathlib.Path]:
    """Find nested .git dirs/files (submodules, separate repos). Excludes the worktree's own .git."""
    hits: list[pathlib.Path] = []
    for dirpath, dirnames, filenames in os.walk(wt_path):
        # don't descend into the worktree's own .git
        rel = pathlib.Path(dirpath).relative_to(wt_path)
        if str(rel) == ".git":
            dirnames[:] = []
            continue
        # don't descend into other VCs
        if ".git" in dirnames or ".git" in filenames:
            # but skip the worktree's own top-level .git
            if str(rel) == ".":
                continue
            hits.append(pathlib.Path(dirpath) / ".git")
            # don't descend further into the nested repo
            if ".git" in dirnames:
                dirnames.remove(".git")
    return hits


def is_worktree_clean(wt_path: pathlib.Path) -> tuple[bool, str]:
    out = subprocess.run(
        ["git", "-C", str(wt_path), "-c", "status.showUntrackedFiles=all",
         "status", "--porcelain", "-z"],
        capture_output=True, text=True, check=True,
    )
    if out.stdout:
        # first entry only for the reason
        first = out.stdout.split("\0")[0]
        return False, first
    return True, ""


# ---------------------------------------------------------------------------
# Lane H


def run_lane_h(
    repo_root: pathlib.Path, config: Config, *,
    apply: bool, keep: set[str], quiet: bool,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    if not config.enabled:
        return records
    trunk_sha = resolve_trunk_sha(repo_root, config.trunk_branch, config.remote_name)
    if not trunk_sha:
        return records
    cur = current_branch(repo_root)
    branches = list_local_branches(repo_root)
    bound = worktree_bound_branches(repo_root)

    skip_names = {config.trunk_branch, "master", cur} | keep

    for br in branches:
        if br.name in skip_names:
            continue
        if not SHA_RE.match(br.sha):
            continue
        if br.name in bound:
            records.append({"lane": "H", "branch": br.name, "tip_sha": br.sha,
                            "action": "skipped-worktree-bound",
                            "reason": "Lane W candidate"})
            continue
        if not is_ancestor(repo_root, br.sha, trunk_sha):
            continue
        if apply:
            backup = write_backup_ref(repo_root, br.name, br.sha)
            ok, err = delete_branch_with_d(repo_root, br.name)
            action = "deleted" if ok else "delete-refused"
            reason = "" if ok else err
            records.append({"lane": "H", "branch": br.name, "tip_sha": br.sha,
                            "action": action, "reason": reason, "backup_ref": backup,
                            "recovery_hint": {"type": "ref", "ref": backup, "sha": br.sha}})
        else:
            records.append({"lane": "H", "branch": br.name, "tip_sha": br.sha,
                            "action": "would-delete", "reason": "ancestor of trunk"})
    if not quiet:
        _print_lane_summary("H", records, apply)
    return records


# ---------------------------------------------------------------------------
# Lane R


def run_lane_r(
    repo_root: pathlib.Path, config: Config, *,
    apply: bool, keep: set[str], quiet: bool,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    if not config.enabled:
        return records
    trunk_sha = resolve_trunk_sha(repo_root, config.trunk_branch, config.remote_name)
    if not trunk_sha:
        return records
    cur = current_branch(repo_root)
    branches = list_local_branches(repo_root)
    bound = worktree_bound_branches(repo_root)
    skip_names = {config.trunk_branch, "master", cur} | keep

    gh_unavailable = False
    for br in branches:
        if br.name in skip_names:
            continue
        if not SHA_RE.match(br.sha):
            continue
        if br.name in bound:
            records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                            "action": "skipped-worktree-bound",
                            "reason": "Lane W candidate"})
            continue
        # ancestor path (Lane H may have skipped due to no hook fired yet)
        if is_ancestor(repo_root, br.sha, trunk_sha):
            if apply:
                backup = write_backup_ref(repo_root, br.name, br.sha)
                ok, err = delete_branch_with_d(repo_root, br.name)
                records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                "action": "deleted" if ok else "delete-refused",
                                "reason": "" if ok else err, "backup_ref": backup,
                                "recovery_hint": {"type": "ref", "ref": backup, "sha": br.sha}})
            else:
                records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                "action": "would-delete", "reason": "ancestor of trunk"})
            continue
        # gh path
        prs, err = gh_merged_pr_for_branch(repo_root, br.name, config.lane_r.gh_timeout_s)
        if prs is None:
            gh_unavailable = True
            records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                            "action": "skipped-gh-unavailable", "reason": err or "gh error"})
            continue
        match = find_matching_merged_pr(prs, br.sha, config.trunk_branch, config.origin_owner)
        if match is None:
            continue
        if apply:
            backup = write_backup_ref(repo_root, br.name, br.sha)
            ok, err = delete_branch_cas(repo_root, br.name, br.sha)
            action = "deleted" if ok else "delete-cas-refused"
            records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                            "action": action, "reason": "" if ok else err,
                            "backup_ref": backup, "pr_number": match.get("number"),
                            "recovery_hint": {"type": "ref", "ref": backup, "sha": br.sha}})
        else:
            records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                            "action": "would-delete",
                            "reason": f"gh PR #{match.get('number')} merged headRefOid==tip",
                            "pr_number": match.get("number")})

    if gh_unavailable and not quiet:
        print(f"[{SCRIPT_NAME}] lane R: gh unavailable for some branches; kept them", file=sys.stderr)
    if not quiet:
        _print_lane_summary("R", records, apply)
    return records


# ---------------------------------------------------------------------------
# Lane W


def _quarantine_target(config: Config, repo_root: pathlib.Path, name: str, sha: str) -> pathlib.Path:
    base = _resolve_path(repo_root, config.lane_w.quarantine_dir)
    ts = int(time.time())
    safe = re.sub(r"[^A-Za-z0-9._-]", "_", name)
    return base / f"{safe}-{sha[:12]}-{ts}"


def _archive_worktree(wt_path: pathlib.Path, archive_path: pathlib.Path) -> tuple[bool, str]:
    """Tar the worktree dir to archive_path. Returns (ok, error)."""
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    # tar from the parent so the archive contains the worktree basename
    parent = wt_path.parent
    name = wt_path.name
    cmd = ["tar", "-czf", str(archive_path), "-C", str(parent), name]
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    except subprocess.TimeoutExpired:
        return False, "tar timeout"
    if out.returncode != 0:
        return False, out.stderr.strip()[:200]
    # integrity check
    verify = subprocess.run(["tar", "-tzf", str(archive_path)],
                            capture_output=True, text=True, timeout=30)
    if verify.returncode != 0:
        try:
            archive_path.unlink(missing_ok=True)
        except OSError:
            pass
        return False, f"archive integrity check failed: {verify.stderr.strip()[:200]}"
    return True, ""


def _lane_w_eligible(
    repo_root: pathlib.Path, config: Config, *,
    branch: str | None, head: str, trunk_sha: str,
) -> tuple[bool, str, dict[str, Any] | None]:
    """Verify Lane H/R eligibility from inside Lane W (ref still exists)."""
    if branch is None:
        # detached: HEAD must be ancestor of trunk
        if is_ancestor(repo_root, head, trunk_sha):
            return True, "detached HEAD ancestor of trunk", None
        return False, "detached HEAD not ancestor of trunk", None
    if is_ancestor(repo_root, head, trunk_sha):
        return True, "ancestor of trunk", None
    # gh path (Lane W is operator-invoked; network is acceptable here)
    prs, err = gh_merged_pr_for_branch(repo_root, branch, config.lane_r.gh_timeout_s)
    if prs is None:
        return False, f"not ancestor and gh unavailable ({err})", None
    match = find_matching_merged_pr(prs, head, config.trunk_branch, config.origin_owner)
    if match is None:
        return False, "no matching merged PR for tip SHA", None
    return True, f"gh PR #{match.get('number')} merged headRefOid==tip", match


def run_lane_w(
    repo_root: pathlib.Path, config: Config, *,
    apply: bool, keep: set[str], quiet: bool,
    discard_ignored: bool, remove_nested: bool, discard_hidden: bool,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    if not config.enabled:
        return records
    trunk_sha = resolve_trunk_sha(repo_root, config.trunk_branch, config.remote_name)
    if not trunk_sha:
        return records
    cur = current_branch(repo_root)
    skip_branches = {config.trunk_branch, "master", cur} | keep

    worktrees = list_worktrees(repo_root)
    main_common = git_common_dir(repo_root)
    main_root = main_common.parent

    lock_path = main_common / LOCK_FILE
    fd = _acquire_lock(lock_path)
    if fd is None:
        if not quiet:
            print(f"[{SCRIPT_NAME}] lane W: another instance holds the lock; aborting",
                  file=sys.stderr)
        return records

    try:
        for wt in worktrees:
            # skip main checkout and currently active worktree
            if wt.path == main_root or wt.branch == cur:
                continue
            if wt.branch in skip_branches:
                continue
            label = wt.branch or f"detached-{wt.head[:8]}"
            eligible, reason, match = _lane_w_eligible(
                repo_root, config, branch=wt.branch, head=wt.head, trunk_sha=trunk_sha,
            )
            if not eligible:
                records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path), "action": "skipped-not-eligible",
                                "reason": reason})
                continue

            # Filesystem guards (refuse-by-default)
            hidden = has_hidden_index_bits(wt.path)
            if hidden and not discard_hidden:
                records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path), "action": "refused-hidden-index-bits",
                                "reason": f"{len(hidden)} file(s) with assume-unchanged/skip-worktree"})
                continue
            ignored = has_ignored_content(wt.path)
            if ignored and not discard_ignored:
                records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path), "action": "refused-ignored-content",
                                "reason": f"{len(ignored)} ignored path(s); use --discard-ignored to override"})
                continue
            nested = has_nested_git(wt.path)
            if nested and not remove_nested:
                records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path), "action": "refused-nested-git",
                                "reason": f"{len(nested)} nested .git path(s); use --remove-nested-repos"})
                continue
            clean, clean_reason = is_worktree_clean(wt.path)
            if not clean:
                records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path), "action": "refused-dirty",
                                "reason": f"uncommitted changes: {clean_reason}"})
                continue

            quarantine = _quarantine_target(config, repo_root, label, wt.head)
            if apply:
                # TOCTOU revalidate right before archive (still under our lock)
                clean2, clean_reason2 = is_worktree_clean(wt.path)
                if not clean2:
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": "refused-dirty-toctou",
                                    "reason": f"TOCTOU: {clean_reason2}"})
                    continue
                # prepare quarantine dir; archive + manifest live INSIDE it (one unit per purge)
                quarantine.mkdir(parents=True, exist_ok=True)
                archive_path = quarantine / "worktree.tar.gz"
                # tar the worktree to quarantine + verify integrity
                ok, err = _archive_worktree(wt.path, archive_path)
                if not ok:
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": "archive-failed",
                                    "reason": err, "quarantine_path": str(quarantine)})
                    continue
                # remove the worktree (plain; refuses if dirty — final TOCTOU safety net)
                rm = _git(repo_root, ["worktree", "remove", str(wt.path)], check=False)
                if rm.returncode != 0:
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": "remove-failed-after-archive",
                                    "reason": rm.stderr.strip(),
                                    "quarantine_path": str(quarantine)})
                    continue
                # worktree gone; branch is now unbound -> safe to delete
                backup_ref = None
                if wt.branch:
                    backup_ref = write_backup_ref(repo_root, wt.branch, wt.head)
                    ok_del, err_del = delete_branch_with_force(repo_root, wt.branch)
                    branch_action = "branch-deleted" if ok_del else "branch-delete-failed"
                    err = err_del if not ok_del else ""
                else:
                    branch_action = "no-bound-branch"
                # write manifest alongside the archive (inside quarantine)
                manifest = {
                    "branch": wt.branch, "tip_sha": wt.head,
                    "moved_from": str(wt.path), "archive": str(archive_path),
                    "archived_at": dt.datetime.now(dt.timezone.utc).isoformat(),
                    "eligibility": reason, "worktree_remove_ok": True,
                    "backup_ref": backup_ref,
                }
                (quarantine / "clean-merged.manifest.json").write_text(
                    json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
                records.append({
                    "lane": "W", "branch": label, "tip_sha": wt.head,
                    "worktree": str(wt.path), "quarantine_path": str(quarantine),
                    "action": branch_action, "reason": err, "backup_ref": backup_ref,
                    "recovery_hint": {"type": "quarantine", "path": str(quarantine),
                                      **({"ref": backup_ref, "sha": wt.head} if backup_ref else {})},
                })
            else:
                records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path),
                                "action": "would-archive-and-remove",
                                "reason": reason,
                                "quarantine_path": str(quarantine),
                                "ignored_count": len(ignored),
                                "hidden_index_count": len(hidden),
                                "nested_git_count": len(nested)})
    finally:
        _release_lock(fd)

    if not quiet:
        _print_lane_summary("W", records, apply)
    return records


# ---------------------------------------------------------------------------
# Purge / prune / doctor


def cmd_purge_quarantine(
    repo_root: pathlib.Path, config: Config, *, grace_days: int | None, quiet: bool,
) -> int:
    base = _resolve_path(repo_root, config.lane_w.quarantine_dir)
    if not base.is_dir():
        if not quiet:
            print(f"[{SCRIPT_NAME}] quarantine absent: {base}")
        return 0
    grace = grace_days if grace_days is not None else config.lane_w.quarantine_grace_days
    cutoff = time.time() - grace * 86400
    purged = 0
    skipped = 0
    for child in base.iterdir():
        if not child.is_dir():
            continue
        manifest_file = child / "clean-merged.manifest.json"
        if not manifest_file.is_file():
            skipped += 1
            continue
        try:
            manifest = json.loads(manifest_file.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            skipped += 1
            continue
        if not manifest.get("worktree_remove_ok"):
            skipped += 1
            continue
        try:
            mtime = child.stat().st_mtime
        except OSError:
            continue
        if mtime > cutoff:
            continue
        # within 7d warning was logged at write time; purge now
        import shutil
        shutil.rmtree(child, ignore_errors=True)
        write_audit(repo_root, config, {
            "lane": "W", "branch": manifest.get("branch"),
            "tip_sha": manifest.get("tip_sha"), "action": "quarantine-purged",
            "quarantine_path": str(child),
        })
        purged += 1
    if not quiet:
        print(f"[{SCRIPT_NAME}] purge: {purged} purged, {skipped} skipped (no/failed manifest)")
    return purged


def cmd_prune_backups(
    repo_root: pathlib.Path, config: Config, *, days: int, quiet: bool,
) -> int:
    out = _git(repo_root, ["for-each-ref", "--format=%(refname)\t%(committerdate:unix)",
                            "refs/clean-merged/"])
    cutoff = time.time() - days * 86400
    pruned = 0
    for line in out.stdout.splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) != 2:
            continue
        ref, ts_str = parts
        try:
            ts = int(ts_str)
        except ValueError:
            continue
        if ts > cutoff:
            continue
        _git(repo_root, ["update-ref", "-d", ref], check=False)
        pruned += 1
    if not quiet:
        print(f"[{SCRIPT_NAME}] prune-backups: {pruned} backup refs removed (>{days}d old)")
    return pruned


def cmd_doctor(repo_root: pathlib.Path, config: Config) -> int:
    problems: list[str] = []
    print(f"[{SCRIPT_NAME}] doctor")

    # config
    print(f"  config.enabled           = {config.enabled}")
    print(f"  config.trunk_branch      = {config.trunk_branch}")
    print(f"  config.remote_name       = {config.remote_name}")
    print(f"  config.origin_owner      = {config.origin_owner}")
    if config.origin_owner is None:
        problems.append("origin owner not resolved; Lane R fork-filter falls back to permissive (same-repo check skipped)")

    # trunk resolution
    trunk_sha = resolve_trunk_sha(repo_root, config.trunk_branch, config.remote_name)
    print(f"  trunk_sha                = {trunk_sha}")
    if not trunk_sha:
        problems.append("trunk ref does not resolve")

    # git-common-dir
    common = git_common_dir(repo_root)
    print(f"  git-common-dir           = {common}")

    # hooks install
    active_hooks_dir_raw = _git(repo_root, ["config", "--get", "core.hooksPath"], check=False)
    if active_hooks_dir_raw.returncode == 0 and active_hooks_dir_raw.stdout.strip():
        active_hooks_dir = pathlib.Path(active_hooks_dir_raw.stdout.strip())
        if not active_hooks_dir.is_absolute():
            active_hooks_dir = repo_root / active_hooks_dir
        print(f"  core.hooksPath           = {active_hooks_dir}")
        for h in ("post-merge", "post-checkout", "post-rewrite"):
            hook_file = active_hooks_dir / h
            exists = hook_file.is_file()
            managed = exists and HOOK_MARKER in hook_file.read_text(encoding="utf-8", errors="replace")
            print(f"  hook {h:14s} exists={exists} managed={managed}")
            if not managed:
                problems.append(f"hook {h} not marked managed")
    else:
        print("  core.hooksPath           = (unset; hooks would live in .git/hooks)")
        problems.append("core.hooksPath unset; clean-merged hooks not active")

    # remote.origin.prune
    prune = _git(repo_root, ["config", "--get", "remote.origin.prune"], check=False)
    print(f"  remote.origin.prune      = {prune.stdout.strip() or '(unset)'}")
    if prune.stdout.strip() != "true":
        problems.append("remote.origin.prune != true (run `just setup`)")

    # gh
    gh_check = subprocess.run(["gh", "--version"], capture_output=True, text=True)
    gh_ok = gh_check.returncode == 0
    print(f"  gh available             = {gh_ok}")
    if not gh_ok:
        problems.append("gh CLI not available; Lane R cannot run")

    # heartbeat freshness
    hb = _resolve_path(repo_root, config.logging.heartbeat_path)
    if hb.is_file():
        try:
            hb_ts = dt.datetime.fromisoformat(hb.read_text(encoding="utf-8").strip())
            age = dt.datetime.now(dt.timezone.utc) - hb_ts
            print(f"  heartbeat age            = {age}")
            if age > dt.timedelta(days=7):
                problems.append(f"heartbeat stale ({age}); hook may be silently broken")
        except (ValueError, OSError):
            problems.append("heartbeat present but unreadable")
    else:
        print("  heartbeat                = (none yet)")

    # quarantine disk usage
    q = _resolve_path(repo_root, config.lane_w.quarantine_dir)
    if q.is_dir():
        n = sum(1 for _ in q.iterdir() if _.is_dir())
        print(f"  quarantine entries       = {n} at {q}")
    else:
        print(f"  quarantine               = (absent)")

    # backup refs
    backups_out = _git(repo_root, ["for-each-ref", "refs/clean-merged/"], check=False)
    n_backups = len([l for l in backups_out.stdout.splitlines() if l.strip()])
    print(f"  backup refs              = {n_backups}")

    if problems:
        print()
        print(f"[{SCRIPT_NAME}] {len(problems)} problem(s):")
        for p in problems:
            print(f"  - {p}")
        return 1
    print()
    print(f"[{SCRIPT_NAME}] all green")
    return 0


# ---------------------------------------------------------------------------
# Output


def _print_lane_summary(lane: str, records: list[dict[str, Any]], apply: bool) -> None:
    if not records:
        return
    verb = "applied" if apply else "dry-run"
    print(f"[{SCRIPT_NAME}] lane {lane} {verb}:")
    for r in records:
        action = r.get("action", "?")
        branch = r.get("branch", "?")
        sha = (r.get("tip_sha") or "")[:12]
        extra = ""
        if r.get("worktree"):
            extra += f" wt={r['worktree']}"
        if r.get("quarantine_path"):
            extra += f" -> {r['quarantine_path']}"
        if r.get("reason"):
            extra += f" :: {r['reason'][:120]}"
        print(f"  {action:30s} {branch:40s} {sha}{extra}")


# ---------------------------------------------------------------------------
# CLI


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog=SCRIPT_NAME, description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--apply", action="store_true",
                   help="execute mutations (default: dry-run)")
    p.add_argument("--quiet", action="store_true")
    p.add_argument("--keep", action="append", default=[], metavar="BRANCH",
                   help="never delete this branch (repeatable)")
    p.add_argument("--lane", choices=("h", "r", "w"), help="run a single lane")
    p.add_argument("--reconcile", action="store_true", help="run Lane R (gh)")
    p.add_argument("--include-worktrees", action="store_true", help="run Lane W")
    p.add_argument("--discard-ignored", action="store_true",
                   help="Lane W: override ignored-content refusal")
    p.add_argument("--remove-nested-repos", action="store_true",
                   help="Lane W: override nested-.git refusal")
    p.add_argument("--discard-hidden-index-bits", action="store_true",
                   help="Lane W: override assume-unchanged/skip-worktree refusal")
    p.add_argument("--purge-quarantine", nargs="?", const=-1, type=int, default=None,
                   metavar="DAYS", help="purge quarantined worktrees older than DAYS")
    p.add_argument("--prune-backups", nargs="?", const=-1, type=int, default=None,
                   metavar="DAYS", help="prune backup refs older than DAYS")
    p.add_argument("--doctor", action="store_true", help="run diagnostic")
    return p


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    repo_root = _resolve_repo_root(pathlib.Path.cwd())
    try:
        config = load_config(repo_root)
    except ConfigError as exc:
        # Don't crash the hook chain on config errors; just warn.
        if not args.quiet:
            print(f"[{SCRIPT_NAME}] config error: {exc}", file=sys.stderr)
        return 0

    if os.environ.get("CLEAN_MERGED_DISABLED") == "1" or not config.enabled:
        if not args.quiet:
            print(f"[{SCRIPT_NAME}] disabled (kill switch)", file=sys.stderr)
        return 0

    keep = set(args.keep)
    apply = args.apply

    # Single-shot subcommands
    if args.doctor:
        return cmd_doctor(repo_root, config)
    if args.purge_quarantine is not None:
        grace = None if args.purge_quarantine == -1 else args.purge_quarantine
        cmd_purge_quarantine(repo_root, config, grace_days=grace, quiet=args.quiet)
        return 0
    if args.prune_backups is not None:
        days = config.backups.prune_after_days if args.prune_backups == -1 else args.prune_backups
        cmd_prune_backups(repo_root, config, days=days, quiet=args.quiet)
        return 0

    write_heartbeat(repo_root, config)

    all_records: list[dict[str, Any]] = []

    if args.lane == "h":
        all_records += run_lane_h(repo_root, config, apply=apply, keep=keep, quiet=args.quiet)
    elif args.lane == "r":
        all_records += run_lane_r(repo_root, config, apply=apply, keep=keep, quiet=args.quiet)
    elif args.lane == "w":
        all_records += run_lane_w(
            repo_root, config, apply=apply, keep=keep, quiet=args.quiet,
            discard_ignored=args.discard_ignored,
            remove_nested=args.remove_nested_repos,
            discard_hidden=args.discard_hidden_index_bits,
        )
    else:
        # default mode: lane H + (lane R if --reconcile)
        all_records += run_lane_h(repo_root, config, apply=apply, keep=keep, quiet=args.quiet)
        if args.reconcile:
            all_records += run_lane_r(repo_root, config, apply=apply, keep=keep, quiet=args.quiet)
        if args.include_worktrees:
            all_records += run_lane_w(
                repo_root, config, apply=apply, keep=keep, quiet=args.quiet,
                discard_ignored=args.discard_ignored,
                remove_nested=args.remove_nested_repos,
                discard_hidden=args.discard_hidden_index_bits,
            )

    # audit successful mutations
    for r in all_records:
        if r.get("action") in ("deleted", "branch-deleted"):
            write_audit(repo_root, config, r)
        elif r.get("action") in ("delete-refused", "delete-cas-refused",
                                  "move-failed", "branch-delete-failed"):
            write_audit(repo_root, config, r)

    return 0


if __name__ == "__main__":
    sys.exit(main())
