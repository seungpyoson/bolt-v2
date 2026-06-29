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
    tar + verify + `git worktree remove`, then CAS `update-ref -d` with the
    re-read tip. Fail-closed on ignored content, assume-unchanged/skip-worktree
    bits (BOTH lowercase ls-files -v flags AND uppercase `S`), nested `.git`,
    dirty. Re-validates hidden-bits + ignored at the TOCTOU point too.

See docs/ops/clean-merged-design.md for the full design and accepted risks.
Config lives in config/clean-merged.toml (single source of truth).
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import fcntl
import hashlib
import json
import math
import os
import pathlib
import re
import shutil
import subprocess
import sys
import time
try:
    import tomllib  # Python 3.11+
    _HAS_TOMLLIB = True
except ImportError:  # pragma: no cover - exercised on Python < 3.11
    _HAS_TOMLLIB = False
from typing import Any

SCRIPT_NAME = "clean-merged"
HOOK_MARKER = f"# {SCRIPT_NAME}-managed"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHORT_SHA_RE = re.compile(r"^[0-9a-f]{7,40}$")
LOCK_FILE = "clean-merged.lock"
# Internal marker that _lane_w_eligible prefixes onto the reason for a refused
# detached-HEAD worktree; run_lane_w strips it and maps it to the distinct
# 'refused-detached-head' action. Single source of truth shared by the producer
# and the consumer (round-soundness-fix review: a one-sided edit to a duplicated
# literal would silently drop the label AND leak the marker into the
# operator-facing reason).
_REFUSED_DETACHED_SENTINEL = "__REFUSED_DETACHED_HEAD__:"


def _parse_toml_flat(text: str) -> dict[str, Any]:
    """Minimal TOML reader for clean-merged's flat schema (sections + scalar keys).

    Used as a fallback when stdlib tomllib is unavailable (Python < 3.11), so the
    tool still runs on system Python without silently no-op'ing inside a hook.
    Handles: section headers [a.b.c], key = value where value is bool/string/int/float.
    NOT a general TOML parser — deliberately restricted to our config's shape.
    """
    root: dict[str, Any] = {}
    cur: dict[str, Any] = root
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip()
            parts = section.split(".")
            node: dict[str, Any] = root
            for p in parts:
                p = p.strip()
                if p not in node or not isinstance(node[p], dict):
                    node[p] = {}
                node = node[p]
            cur = node
            continue
        if "=" not in line:
            raise ValueError(f"invalid TOML line: {raw_line!r}")
        key, _, value = line.partition("=")
        key = key.strip()
        value = value.strip()
        # Strip inline comments OUTSIDE quoted strings (round-4 P2 by Kimi/GPT:
        # a # inside "foo # bar" was corrupting the value).
        if value and value[0] in "\"'":
            quote = value[0]
            end = value.find(quote, 1)
            if end == -1:
                raise ValueError(f"unterminated string: {raw_line!r}")
            # Keep the quoted token; trailing data must be a comment or whitespace.
            value = value[: end + 1]
        else:
            # Bare value: cut at the first '#' that's a comment start.
            hash_idx = value.find("#")
            if hash_idx != -1:
                value = value[:hash_idx].strip()
                if not value:
                    continue
        parsed: Any
        if value == "true":
            parsed = True
        elif value == "false":
            parsed = False
        elif value.startswith('"') and value.endswith('"'):
            parsed = value[1:-1]
        elif value.startswith("'") and value.endswith("'"):
            parsed = value[1:-1]
        elif value.startswith("[") and value.endswith("]"):
            # array of scalars (round-4 P2: future-proofs against `key = ["a", "b"]`).
            inner = value[1:-1].strip()
            if not inner:
                parsed = []
            else:
                items: list[Any] = []
                for raw_item in inner.split(","):
                    item = raw_item.strip()
                    if (item.startswith('"') and item.endswith('"')) or \
                       (item.startswith("'") and item.endswith("'")):
                        items.append(item[1:-1])
                    else:
                        try:
                            items.append(int(item))
                        except ValueError:
                            try:
                                items.append(float(item))
                            except ValueError:
                                raise ValueError(f"unsupported TOML array item: {item!r}")
                parsed = items
        else:
            try:
                parsed = int(value)
            except ValueError:
                try:
                    parsed = float(value)
                except ValueError:
                    raise ValueError(f"unsupported TOML value: {value!r}")
        cur[key] = parsed
    return root


def _load_toml(path: pathlib.Path) -> dict[str, Any]:
    """Load TOML via stdlib if available, else fall back to the flat-schema parser."""
    text = path.read_text(encoding="utf-8")
    if _HAS_TOMLLIB:
        return tomllib.loads(text)
    return _parse_toml_flat(text)



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
    """Resolve the MAIN worktree root (where config/docs live), not a feature worktree.

    Round-5 (GPT/Claude/Gemini/Kimi P1): the prior implementation assumed
    `git rev-parse --git-common-dir` returns `<main>/.git`, then took `.parent`
    to get `<main>`. That's wrong inside a SUBMODULE, where --git-common-dir
    returns `<super>/.git/modules/<name>` (its parent is INSIDE the superproject
    git dir, not a working tree).

    Hardened approach: parse `git worktree list --porcelain` and return the
    FIRST worktree's path. Idempotent and correct for normal repos, linked
    worktrees, and submodules-in-their-own-working-tree.

    Limitation (round-5.5 Claude F-C1, documented): for a SUBMODULE, this
    returns the submodule's main worktree (correct), but `load_config` then
    looks for `config/clean-merged.toml` there. Most submodules don't have
    one → ConfigError → safe no-op (main returns 0; hook's || true handles it).
    The tool does NOT pick up the superproject's config and does NOT operate
    on superproject refs from inside a submodule. Documented as a known limit;
    full submodule support deferred behind an explicit future slice.
    """
    try:
        out = subprocess.run(
            ["git", "worktree", "list", "--porcelain"],
            cwd=repo_root, check=True, capture_output=True, text=True,
        )
        for line in out.stdout.splitlines():
            if line.startswith("worktree "):
                candidate = pathlib.Path(line[len("worktree "):].strip())
                if candidate.is_dir():
                    if (candidate / ".git").exists():
                        return candidate
                    try:
                        worktree_val = subprocess.run(
                            ["git", "-C", str(candidate), "config", "--get", "core.worktree"],
                            check=True, capture_output=True, text=True,
                        ).stdout.strip()
                        if worktree_val:
                            resolved = (candidate / worktree_val).resolve()
                            if (resolved / ".git").exists() or (resolved.is_dir() and (resolved / "config").exists()):
                                return resolved
                    except subprocess.CalledProcessError:
                        pass
    except subprocess.CalledProcessError:
        pass
    # Fallback for normal repos / linked worktrees (not submodules).
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            cwd=repo_root, check=True, capture_output=True, text=True,
        )
        common_dir = pathlib.Path(out.stdout.strip())
        candidate = common_dir.parent
        # Sanity check: parent should be a working tree, not inside .git/.
        if (candidate / ".git").exists() or (candidate.is_dir() and (candidate / "config").exists()):
            return candidate
    except subprocess.CalledProcessError:
        pass
    return repo_root


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
    # SSH scp-style: git@github.com:owner/repo.git
    m = re.match(r"git@[^:]+:([^/]+)/([^/]+?)(?:\.git)?$", url)
    if m:
        return m.group(1)
    # ssh:// or git+ssh://: ssh://git@github.com/owner/repo(.git)?
    m = re.match(r"(?:git\+)?ssh://(?:[^@]+@)?[^/]+/([^/]+)/([^/]+?)(?:\.git)?$", url)
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
        data = _load_toml(cfg_path)
    except (tomllib.TOMLDecodeError if _HAS_TOMLLIB else ValueError) as exc:
        raise ConfigError(f"invalid TOML in {cfg_path}: {exc}") from exc

    enabled = bool(_get_nested(data, "clean-merged.enabled", True))
    trunk_branch = str(_get_nested(data, "clean-merged.trunk_branch", "main"))
    remote_name = str(_get_nested(data, "clean-merged.remote_name", "origin"))

    lane_r = LaneRConfig(
        gh_timeout_s=float(_get_nested(data, "clean-merged.lane_r.gh_timeout_s", 5.0)),
        cache_ttl_s=float(_get_nested(data, "clean-merged.lane_r.cache_ttl_s", 300.0)),
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
        lane_r=lane_r, lane_w=lane_w,
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


def delete_branch_with_d(repo_root: pathlib.Path, branch: str, expected_sha: str) -> tuple[bool, str]:
    """Lane H/R: CAS delete via `git update-ref -d refs/heads/<branch> <expected_sha>`.

    We do NOT use `git branch -d` because its merged-ness check is against HEAD
    or the branch's upstream — not the trunk we already verified ancestor-against.
    When Lane H runs from a hook while HEAD is on a feature branch (or behind
    trunk), `branch -d` may refuse eligible branches (Claude/GPT round-4 P1-2).
    The is_ancestor(<B>, <trunk>) check above already proved merged-ness; CAS
    deletes exactly that tip and refuses on SHA drift.
    """
    out = _git(repo_root, ["update-ref", "-d", f"refs/heads/{branch}", expected_sha],
               check=False)
    return out.returncode == 0, out.stderr.strip()


def delete_branch_with_force(repo_root: pathlib.Path, branch: str) -> tuple[bool, str]:
    """Lane W post-move (legacy): `git branch -D` after explicit eligibility verification.

    Prefer delete_branch_with_force_cas (CAS) for new code paths — it refuses
    on SHA drift, defending against the round-4 P0 where a commit lands in the
    worktree between list_worktrees() and branch deletion.
    """
    out = _git(repo_root, ["branch", "-D", branch], check=False)
    return out.returncode == 0, out.stderr.strip()


def delete_branch_with_force_cas(repo_root: pathlib.Path, branch: str,
                                  expected_sha: str) -> tuple[bool, str]:
    """Lane W post-remove: CAS delete via `git update-ref -d <ref> <verified_sha>`.

    Used after the worktree is gone and we've just re-read the branch tip.
    Refuses on SHA drift, defending against a commit landing in the worktree
    between list_worktrees() and now (round-4 P0 by Kimi/GPT).
    """
    out = _git(repo_root, ["update-ref", "-d", f"refs/heads/{branch}", expected_sha],
               check=False)
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
    """Acquire fcntl.flock on lock_path. Returns fd, or None if it would block.

    Round-4 P2 (Claude): the exclusive case previously had no LOCK_NB, so it
    blocked indefinitely (the "another instance holds the lock; aborting"
    branch was dead code). Now non-blocking in both modes.
    """
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(str(lock_path), os.O_CREAT | os.O_RDWR, 0o644)
    try:
        flags = (fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH) | fcntl.LOCK_NB
        fcntl.flock(fd, flags)
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


def _atomic_write_text(path: pathlib.Path, text: str) -> None:
    """Atomic write via tmp + os.replace (round-5 P1 by GPT/Kimi/Grok).

    Plain pathlib.Path.write_text() truncates-then-writes; an interruption
    leaves the file empty or partial JSON. For manifests whose integrity gates
    purge decisions, that's a data-loss vector. Atomic rename guarantees the
    file is either the previous content or the new content, never partial.

    Round-5.5 (Kimi/Grok P2): try/finally unlinks the tmp file if we crash
    between write_text and os.replace, so orphan .tmp.<pid> files don't
    accumulate across many crashes.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.tmp.{os.getpid()}")
    try:
        tmp.write_text(text, encoding="utf-8")
        os.replace(str(tmp), str(path))
    except BaseException:
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass
        raise


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
_GH_CACHE_NAME = "clean-merged-gh-cache.json"


def _gh_cache_path(repo_root: pathlib.Path) -> pathlib.Path:
    return git_common_dir(repo_root) / _GH_CACHE_NAME


def _load_gh_cache(path: pathlib.Path) -> dict[str, Any]:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}


def _save_gh_cache(path: pathlib.Path, cache: dict[str, Any], ttl: float) -> None:
    """Atomic write via tmp + os.replace (round-4 P2 by Kimi/Grok/GPT) +
    TTL-based eviction (round-5 P2 by GPT/Claude/Gemini/Kimi/Grok).

    Concurrent detached Lane R processes RMW the cache; non-atomic
    path.write_text() can interleave/truncate. Atomic rename makes the worst
    case a lost update (one writer wins), never corruption.

    Additionally, every save prunes:
      - entries whose fetched_at is older than TTL (no point keeping them)
      - entries without '@' in the key (pre-round-4.5 migration cruft, keyed
        by branch alone — never read by the new code)
      - entries with malformed/non-finite fetched_at (round-5.5 GPT/Claude P2)
    This bounds cache growth under sustained rebase/force-push churn.

    Round-5.5 polish (NO DUAL PATHS): uses the shared _entry_is_live helper so
    the read path and the prune path cannot disagree on what counts as live.
    """
    now = time.time()
    # _entry_is_live already covers @-in-key + dict-shape; we don't need a
    # separate "@" check here.
    pruned = {k: v for k, v in cache.items() if _entry_is_live(v, now, ttl) and "@" in k}
    try:
        tmp = path.with_suffix(path.suffix + f".tmp.{os.getpid()}")
        tmp.write_text(json.dumps(pruned), encoding="utf-8")
        os.replace(str(tmp), str(path))
    except OSError:
        pass


def gh_merged_pr_for_branch(
    repo_root: pathlib.Path, branch: str, timeout: float,
) -> tuple[list[dict[str, Any]] | None, str | None]:
    """Return (prs, error). prs=None means gh trouble (keep the branch)."""
    cmd = [
        # --limit 100 (round-4.5: was 5; Kimi P1 noted that a branch reused
        # >5x with the current tip = older PR would be missed. 100 covers any
        # realistic reuse pattern; the headRefOid match makes false-positives
        # impossible regardless of how many PRs are returned.)
        "gh", "pr", "list", "--head", branch, "--state", "merged",
        "--json", "number,headRefOid,baseRefName,headRepositoryOwner,isCrossRepository",
        "--limit", "100",
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


def _entry_is_live(entry: Any, now: float, ttl: float) -> bool:
    """Single source of truth for "is this cache entry fresh enough to use?"

    Used by both the read path (gh_merged_pr_for_branch_cached) and the prune
    path (_save_gh_cache) so they cannot disagree (NO DUAL PATHS).

    Round-5.5 polish (GPT P2 #1, Claude P3-2): catch TypeError/ValueError/
    OverflowError on float() AND reject non-finite values (inf/nan). A
    corrupted/tampered cache entry like {"fetched_at": "not-a-float"} or
    {"fetched_at": inf} is treated as expired (drop on save; miss on read
    → fresh gh call → write pruned cache → self-heals).
    """
    if not isinstance(entry, dict):
        return False
    try:
        age = now - float(entry.get("fetched_at", 0))
    except (TypeError, ValueError, OverflowError):
        return False
    if not math.isfinite(age):
        return False
    return age < ttl


def gh_merged_pr_for_branch_cached(
    repo_root: pathlib.Path, config: Config, branch: str, tip: str,
) -> tuple[list[dict[str, Any]] | None, str | None]:
    """Per-branch gh result with TTL cache (avoids re-querying on every hook fire).

    Cache key is (branch, tip-sha[:12]) — round-4.5 self-review / Grok P2: keyed
    by branch alone, a stale negative result (no merged PR for tip A) would
    suppress cleanup for up to TTL after tip advances to a merged squash commit B.
    Keying by (branch, tip) invalidates the entry automatically when the tip moves.

    Stored under <git-common-dir>/clean-merged-gh-cache.json. On any I/O trouble
    the cache is bypassed (fail-open to a fresh gh call), never fail-closed.
    """
    cache_path = _gh_cache_path(repo_root)
    now = time.time()
    cache = _load_gh_cache(cache_path)
    cache_key = f"{branch}@{tip[:12]}"
    entry = cache.get(cache_key)
    if _entry_is_live(entry, now, config.lane_r.cache_ttl_s):
        return entry.get("prs"), entry.get("error")
    prs, err = gh_merged_pr_for_branch(repo_root, branch, config.lane_r.gh_timeout_s)
    cache[cache_key] = {"fetched_at": now, "prs": prs, "error": err}
    _save_gh_cache(cache_path, cache, config.lane_r.cache_ttl_s)
    return prs, err


def find_matching_merged_pr(
    prs: list[dict[str, Any]], tip: str, trunk: str, origin_owner: str | None,
) -> dict[str, Any] | None:
    """Return the PR whose headRefOid == tip AND baseRefName == trunk AND same-repo.

    Round-4 P2 (Claude P2-E): when resolve_trunk_sha falls back master↔main,
    the config.trunk_branch name no longer matches the actual trunk. Accept
    either name in the baseRefName filter so the fallback doesn't silently
    suppress all matches.
    """
    fallback_trunk = "master" if trunk == "main" else "main"
    acceptable_bases = {trunk, fallback_trunk}
    for pr in prs:
        head_oid = pr.get("headRefOid")
        if not isinstance(head_oid, str) or not SHA_RE.match(head_oid):
            continue
        if head_oid != tip:
            continue
        if pr.get("baseRefName") not in acceptable_bases:
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
    """Return paths with assume-unchanged/skip-worktree bits.

    `git ls-files -v` uses lowercase letters for assume-unchanged (e.g. `h`
    instead of `H` for cached) and UPPERCASE `S` for skip-worktree. Both bits
    hide modifications from `git status --porcelain`, so both must be detected
    or Lane W will archive+remove worktrees with hidden dirty state.

    Per `git help ls-files`: identified flags are c/m/k/? for various states,
    H for cached; the lowercase variant means assume-unchanged is set; the
    dedicated skip-worktree marker is uppercase S. We flag any line whose
    first char is a lowercase alpha (assume-unchanged) OR an uppercase S.
    """
    out = subprocess.run(
        ["git", "-C", str(wt_path), "ls-files", "-v"],
        capture_output=True, text=True, check=True,
    )
    flagged: list[str] = []
    for line in out.stdout.splitlines():
        if not line:
            continue
        flag = line[0]
        # lowercase alpha = assume-unchanged bit on a tracked file
        # uppercase 'S' = skip-worktree bit on a tracked file
        if (flag.islower() and flag.isalpha()) or flag == "S":
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

    skip_names = {b for b in (config.trunk_branch, "master", cur) if b} | keep

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
            # Re-read tip immediately before delete in case the branch moved
            # between list_local_branches() above and now. CAS refuses on drift.
            fresh_tip = _git(repo_root, ["rev-parse", br.name], check=False).stdout.strip()
            if not fresh_tip or not SHA_RE.match(fresh_tip):
                continue
            if fresh_tip != br.sha:
                records.append({"lane": "H", "branch": br.name, "tip_sha": br.sha,
                                "action": "skipped-tip-moved",
                                "reason": f"tip drifted {br.sha[:12]} -> {fresh_tip[:12]}"})
                continue
            # Re-verify worktree binding (round-4 P0): a worktree may have been
            # bound to this branch between function entry and the CAS delete.
            fresh_bound = worktree_bound_branches(repo_root)
            if br.name in fresh_bound:
                records.append({"lane": "H", "branch": br.name, "tip_sha": br.sha,
                                "action": "skipped-worktree-bound-toctou",
                                "reason": "branch became worktree-bound after eligibility check"})
                continue
            backup = write_backup_ref(repo_root, br.name, fresh_tip)
            ok, err = delete_branch_with_d(repo_root, br.name, fresh_tip)
            action = "deleted" if ok else "delete-cas-refused"
            reason = "" if ok else err
            records.append({"lane": "H", "branch": br.name, "tip_sha": fresh_tip,
                            "action": action, "reason": reason, "backup_ref": backup,
                            "recovery_hint": {"type": "ref", "ref": backup, "sha": fresh_tip}})
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
    skip_names = {b for b in (config.trunk_branch, "master", cur) if b} | keep

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
                fresh_tip = _git(repo_root, ["rev-parse", br.name], check=False).stdout.strip()
                if not fresh_tip or not SHA_RE.match(fresh_tip):
                    continue
                if fresh_tip != br.sha:
                    records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                    "action": "skipped-tip-moved",
                                    "reason": f"tip drifted {br.sha[:12]} -> {fresh_tip[:12]}"})
                    continue
                fresh_bound = worktree_bound_branches(repo_root)
                if br.name in fresh_bound:
                    records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                    "action": "skipped-worktree-bound-toctou",
                                    "reason": "branch became worktree-bound after eligibility"})
                    continue
                backup = write_backup_ref(repo_root, br.name, fresh_tip)
                ok, err = delete_branch_with_d(repo_root, br.name, fresh_tip)
                records.append({"lane": "R", "branch": br.name, "tip_sha": fresh_tip,
                                "action": "deleted" if ok else "delete-cas-refused",
                                "reason": "" if ok else err, "backup_ref": backup,
                                "recovery_hint": {"type": "ref", "ref": backup, "sha": fresh_tip}})
            else:
                records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                "action": "would-delete", "reason": "ancestor of trunk"})
            continue
        # gh path
        prs, err = gh_merged_pr_for_branch_cached(repo_root, config, br.name, br.sha)
        if prs is None:
            gh_unavailable = True
            records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                            "action": "skipped-gh-unavailable", "reason": err or "gh error"})
            continue
        match = find_matching_merged_pr(prs, br.sha, config.trunk_branch, config.origin_owner)
        if match is None:
            continue
        if apply:
            fresh_tip = _git(repo_root, ["rev-parse", br.name], check=False).stdout.strip()
            if not fresh_tip or not SHA_RE.match(fresh_tip):
                continue
            if fresh_tip != br.sha:
                records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                "action": "skipped-tip-moved",
                                "reason": f"tip drifted {br.sha[:12]} -> {fresh_tip[:12]}"})
                continue
            fresh_bound = worktree_bound_branches(repo_root)
            if br.name in fresh_bound:
                records.append({"lane": "R", "branch": br.name, "tip_sha": br.sha,
                                "action": "skipped-worktree-bound-toctou",
                                "reason": "branch became worktree-bound after gh match"})
                continue
            backup = write_backup_ref(repo_root, br.name, fresh_tip)
            ok, err = delete_branch_cas(repo_root, br.name, fresh_tip)
            action = "deleted" if ok else "delete-cas-refused"
            records.append({"lane": "R", "branch": br.name, "tip_sha": fresh_tip,
                            "action": action, "reason": "" if ok else err,
                            "backup_ref": backup, "pr_number": match.get("number"),
                            "recovery_hint": {"type": "ref", "ref": backup, "sha": fresh_tip}})
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


def _quarantine_target(config: Config, repo_root: pathlib.Path, name: str, sha: str,
                        wt_path: pathlib.Path) -> pathlib.Path:
    base = _resolve_path(repo_root, config.lane_w.quarantine_dir)
    ts = int(time.time())
    pid = os.getpid()
    # Hash the absolute worktree path to disambiguate multiple worktrees bound
    # to the same branch (rare but possible). Without it, two same-second
    # archives of the same branch would collide on the same quarantine dir.
    wt_hash = hashlib.sha1(str(wt_path).encode("utf-8")).hexdigest()[:8]
    safe = re.sub(r"[^A-Za-z0-9._-]", "_", name)
    return base / f"{safe}-{sha[:12]}-{ts}-{pid}-{wt_hash}"


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
    # integrity check (round-5.5: catch TimeoutExpired — a malformed/huge
    # archive could hang tar, and uncaught it would propagate out of Lane W's
    # per-iteration except as a crash here, or out of cmd_purge_quarantine
    # entirely killing the rest of the sweep).
    try:
        verify = subprocess.run(["tar", "-tzf", str(archive_path)],
                                capture_output=True, text=True, timeout=30)
        if verify.returncode != 0:
            try:
                archive_path.unlink(missing_ok=True)
            except OSError:
                pass
            return False, f"archive integrity check failed: {verify.stderr.strip()[:200]}"
    except subprocess.TimeoutExpired:
        try:
            archive_path.unlink(missing_ok=True)
        except OSError:
            pass
        return False, "archive integrity check timed out (possible malformed archive)"
    return True, ""


def _lane_w_eligible(
    repo_root: pathlib.Path, config: Config, *,
    branch: str | None, head: str, trunk_sha: str,
    allow_detached_removal: bool = False,
) -> tuple[bool, str, dict[str, Any] | None]:
    """Verify Lane H/R eligibility from inside Lane W (ref still exists).

    Soundness (GPT P0 / Kimi RECOVERY_HOLE, round-soundness): detached-HEAD
    worktrees are REFUSED by default. Scenario defeated: operator makes an
    exploratory commit in a detached worktree, resets back to trunk, then runs
    Lane W. The worktree's HEAD is now at trunk (ancestor-of-trunk → previously
    eligible), but the reset-away commit lives only in the worktree's reflog.
    Lane W's archive captures the post-reset working tree (NOT the orphaned
    commit), `git worktree remove` deletes the worktree's reflog with the
    admin entry, and the commit becomes unreachable — permanently lost after
    gc. Default refuse; require explicit --allow-detached-removal to override
    after accepting that reflog-only commits in detached worktrees will not be
    preserved by the archive.
    """
    if branch is None:
        if not allow_detached_removal:
            # Round-soundness-fix review (Kimi P2 #1): distinct reason so Lane W
            # can label this as 'refused-detached-head' in the audit log instead
            # of burying it under the generic 'skipped-not-eligible' action.
            return (False, _REFUSED_DETACHED_SENTINEL
                    + " detached-HEAD worktree refused "
                    "(use --allow-detached-removal to override; "
                    "reflog-only commits are not preserved by the archive)", None)
        # detached: HEAD must be ancestor of trunk
        if is_ancestor(repo_root, head, trunk_sha):
            return True, "detached HEAD ancestor of trunk (--allow-detached-removal)", None
        return False, "detached HEAD not ancestor of trunk", None
    if is_ancestor(repo_root, head, trunk_sha):
        return True, "ancestor of trunk", None
    # gh path (Lane W is operator-invoked; network is acceptable here)
    prs, err = gh_merged_pr_for_branch_cached(repo_root, config, branch, head)
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
    invoke_root: pathlib.Path | None = None,
    allow_detached_removal: bool = False,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    if not config.enabled:
        return records
    trunk_sha = resolve_trunk_sha(repo_root, config.trunk_branch, config.remote_name)
    if not trunk_sha:
        return records
    # `cur` must reflect the INVOKER's branch, not repo_root's (round-5 Grok P1):
    # if the operator runs Lane W from inside a feature worktree while repo_root
    # resolves to the main worktree, the active-worktree skip would not protect
    # the invoker's worktree.
    invoke_root = invoke_root or repo_root
    cur = current_branch(invoke_root)
    skip_branches = {b for b in (config.trunk_branch, "master", cur) if b} | keep

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
        # Hoist loop invariants (round-5.5 polish: Claude P3-4, Kimi P3).
        main_root_resolved = main_root.resolve()
        invoke_root_resolved = invoke_root.resolve()
        for wt in worktrees:
            wt_path_resolved = wt.path.resolve()
            try:
                # skip main checkout and currently active worktree
                # round-5 Grok P1: also skip the invoker's worktree path
                # explicitly (in case invoke_root's branch resolution missed it
                # — e.g. detached HEAD in the invoker).
                # round-5.5 Grok P2: use .resolve() consistently so macOS
                # /tmp ↔ /private/tmp symlinks can't defeat the check.
                # round-5.5 polish review (Claude P3-1, GPT P2, Kimi P2): restore
                # the `wt.branch is not None` guard. Without it, when the invoker
                # is detached (cur=None), every detached worktree (wt.branch=None)
                # matches via None==None → True → silently skipped. Over-skipping
                # (under-cleaning), not data loss, but a real correctness bug.
                # The detached invoker itself is still protected by the
                # invoke_root.resolve() path check below.
                # round-5.5 polish review (Claude P3-4, Kimi P3): hoist .resolve()
                # calls out of the loop — they're loop invariants.
                if (wt_path_resolved == main_root_resolved
                        or (wt.branch is not None and wt.branch == cur)
                        or wt_path_resolved == invoke_root_resolved):
                    continue
                if wt.branch in skip_branches:
                    continue
                label = wt.branch or f"detached-{wt.head[:8]}"
                # gemini-code-assist P2: if the worktree dir was manually deleted
                # from disk (rm -rf without `git worktree remove`), git commands
                # on the missing path would raise CalledProcessError. Check
                # existence first so the operator gets a clean diagnostic and a
                # hint to run `git worktree prune` to clean up the stale admin
                # entry.
                if not wt.path.is_dir():
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": "skipped-missing-dir",
                                    "reason": "worktree directory does not exist on disk; "
                                              "run `git worktree prune` to clean up the stale admin entry"})
                    continue
                eligible, reason, match = _lane_w_eligible(
                    repo_root, config, branch=wt.branch, head=wt.head, trunk_sha=trunk_sha,
                    allow_detached_removal=allow_detached_removal,
                )
                if not eligible:
                    # Round-soundness-fix review (Kimi P2 #1): map the
                    # detached-refusal sentinel to a distinct action label so
                    # doctor / audit-log queries can find these specifically
                    # rather than digging through generic skipped-not-eligible.
                    if reason.startswith(_REFUSED_DETACHED_SENTINEL):
                        action = "refused-detached-head"
                        reason = reason[len(_REFUSED_DETACHED_SENTINEL):].strip()
                    else:
                        action = "skipped-not-eligible"
                    records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                    "worktree": str(wt.path), "action": action,
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

                quarantine = _quarantine_target(config, repo_root, label, wt.head, wt.path)
                if apply:
                    # TOCTOU revalidate right before archive (still under our lock).
                    # Re-run hidden-bits + ignored too: another process could have
                    # set assume-unchanged/skip-worktree OR dropped an ignored file
                    # in the window since the upfront guards (round-4 P1 by Grok).
                    clean2, clean_reason2 = is_worktree_clean(wt.path)
                    if not clean2:
                        records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                        "worktree": str(wt.path), "action": "refused-dirty-toctou",
                                        "reason": f"TOCTOU: {clean_reason2}"})
                        continue
                    hidden2 = has_hidden_index_bits(wt.path)
                    if hidden2 and not discard_hidden:
                        records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                        "worktree": str(wt.path),
                                        "action": "refused-hidden-index-bits-toctou",
                                        "reason": f"TOCTOU: {len(hidden2)} hidden-bit file(s)"})
                        continue
                    ignored2 = has_ignored_content(wt.path)
                    if ignored2 and not discard_ignored:
                        records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                        "worktree": str(wt.path),
                                        "action": "refused-ignored-content-toctou",
                                        "reason": f"TOCTOU: {len(ignored2)} ignored path(s)"})
                        continue
                    # prepare quarantine dir; archive + manifest live INSIDE it.
                    # Write a minimal manifest IMMEDIATELY so a crash between here
                    # and the final manifest update still leaves a recoverable
                    # trail (round-4 P1 by GPT).
                    quarantine.mkdir(parents=True, exist_ok=True)
                    archive_path = quarantine / "worktree.tar.gz"
                    minimal_manifest = {
                        "branch": wt.branch, "tip_sha_at_archive_time": wt.head,
                        "moved_from": str(wt.path), "archive": str(archive_path),
                        "archived_at": dt.datetime.now(dt.timezone.utc).isoformat(),
                        "eligibility": reason,
                        "worktree_remove_ok": False,        # set True after remove
                        "branch_delete_ok": False,          # set after branch delete
                        "backup_ref": None,                 # set after backup write
                        "final_tip_sha": None,              # set after re-read
                    }
                    _atomic_write_text(quarantine / "clean-merged.manifest.json",
                        json.dumps(minimal_manifest, indent=2, sort_keys=True))
                    # tar the worktree to quarantine + verify integrity
                    ok, err = _archive_worktree(wt.path, archive_path)
                    if not ok:
                        rec = {"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path), "action": "archive-failed",
                                "reason": err, "quarantine_path": str(quarantine)}
                        records.append(rec)
                        write_audit(repo_root, config, rec)
                        continue
                    # remove the worktree (plain; refuses if dirty — final TOCTOU safety net)
                    # Round-5 (Claude/Grok P2): audit the recovery_hint BEFORE the
                    # remove call. Previously the worktree-removed-branch-pending
                    # audit was written AFTER the flip, so a crash between remove
                    # and flip left no recovery_hint pointing at the quarantine.
                    write_audit(repo_root, config, {
                        "lane": "W", "branch": label, "tip_sha": wt.head,
                        "worktree": str(wt.path), "quarantine_path": str(quarantine),
                        "archive_path": str(archive_path),
                        "action": "worktree-removal-attempted",
                        "reason": "worktree archive written; removal attempt imminent",
                        "recovery_hint": {"type": "quarantine", "path": str(quarantine),
                                          "archive": str(archive_path)},
                    })
                    rm = _git(repo_root, ["worktree", "remove", str(wt.path)], check=False)
                    if rm.returncode != 0:
                        rec = {"lane": "W", "branch": label, "tip_sha": wt.head,
                                "worktree": str(wt.path), "action": "remove-failed-after-archive",
                                "reason": rm.stderr.strip(),
                                "quarantine_path": str(quarantine)}
                        records.append(rec)
                        write_audit(repo_root, config, rec)
                        continue
                    # Worktree successfully removed. Flip the manifest's
                    # worktree_remove_ok RIGHT NOW (before branch-delete / final
                    # manifest write) so a crash between here and the end of the
                    # iteration leaves the quarantine entry purge-eligible.
                    # (round-4.5 self-review: previously the minimal manifest
                    # stayed worktree_remove_ok=False until the final write,
                    # so a crash left the entry unpurgeable forever.)
                    minimal_manifest["worktree_remove_ok"] = True
                    minimal_manifest["worktree_removed_at"] = (
                        dt.datetime.now(dt.timezone.utc).isoformat())
                    _atomic_write_text(quarantine / "clean-merged.manifest.json",
                        json.dumps(minimal_manifest, indent=2, sort_keys=True))
                    # Worktree gone. Re-read the branch tip now (round-4 P0 by Kimi/GPT):
                    # a commit may have landed in the worktree between list_worktrees()
                    # and now. If it moved, we must NOT delete with the stale SHA —
                    # the new commit would be lost (the backup ref points to the old tip).
                    fresh_tip = wt.head
                    if wt.branch:
                        fresh_tip_out = _git(repo_root, ["rev-parse", wt.branch], check=False)
                        if fresh_tip_out.returncode == 0:
                            fresh_tip = fresh_tip_out.stdout.strip()
                    # Audit IMMEDIATELY so a crash between here and branch-delete
                    # leaves a forensic trail. Include recovery_hint this time
                    # (round-4 P1 by Kimi).
                    rec_worktree_removed = {
                        "lane": "W", "branch": label, "tip_sha": fresh_tip,
                        "worktree": str(wt.path), "quarantine_path": str(quarantine),
                        "action": "worktree-removed-branch-pending",
                        "reason": "worktree archived and removed; branch delete pending",
                        "recovery_hint": {"type": "quarantine", "path": str(quarantine),
                                          "archive": str(archive_path)},
                    }
                    write_audit(repo_root, config, rec_worktree_removed)
                    backup_ref = None
                    branch_action: str
                    if wt.branch:
                        backup_ref = write_backup_ref(repo_root, wt.branch, fresh_tip)
                        # CAS delete with the FRESH tip (not the stale wt.head).
                        ok_del, err_del = delete_branch_with_force_cas(
                            repo_root, wt.branch, fresh_tip)
                        branch_action = "branch-deleted" if ok_del else "branch-delete-failed"
                        err = err_del if not ok_del else ""
                    else:
                        branch_action = "no-bound-branch"
                    # finalize manifest
                    final_manifest = {
                        **minimal_manifest,
                        "worktree_remove_ok": True,
                        "branch_delete_ok": branch_action == "branch-deleted",
                        "backup_ref": backup_ref,
                        "final_tip_sha": fresh_tip,
                        "tip_drifted": fresh_tip != wt.head,
                    }
                    _atomic_write_text(quarantine / "clean-merged.manifest.json",
                        json.dumps(final_manifest, indent=2, sort_keys=True))
                    rec = {
                        "lane": "W", "branch": label, "tip_sha": fresh_tip,
                        "worktree": str(wt.path), "quarantine_path": str(quarantine),
                        "action": branch_action, "reason": err, "backup_ref": backup_ref,
                        "tip_drifted": fresh_tip != wt.head,
                        "recovery_hint": {"type": "quarantine", "path": str(quarantine),
                                          **({"ref": backup_ref, "sha": fresh_tip} if backup_ref else {})},
                    }
                    records.append(rec)
                    write_audit(repo_root, config, rec)
                else:
                        records.append({"lane": "W", "branch": label, "tip_sha": wt.head,
                                        "worktree": str(wt.path),
                                        "action": "would-archive-and-remove",
                                        "reason": reason,
                                        "quarantine_path": str(quarantine),
                                        "ignored_count": len(ignored),
                                        "hidden_index_count": len(hidden),
                                        "nested_git_count": len(nested)})
            except Exception as exc:  # noqa: BLE001 — never let one worktree kill the sweep
                # Log + continue; never break the lane over a single failure.
                rec = {"lane": "W", "branch": getattr(wt, "branch", None) or "<unknown>",
                        "tip_sha": getattr(wt, "head", ""), "action": "iteration-exception",
                        "reason": f"{type(exc).__name__}: {exc}"}
                records.append(rec)
                try:
                    write_audit(repo_root, config, rec)
                except (OSError, TypeError, ValueError):
                    pass
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
    # Acquire the same lock Lane W holds so a concurrent sweep can't race us
    # mid-manifest-write (round-4 P1 by Kimi).
    main_common = git_common_dir(repo_root)
    lock_path = main_common / LOCK_FILE
    fd = _acquire_lock(lock_path)
    if fd is None:
        if not quiet:
            print(f"[{SCRIPT_NAME}] purge: another instance holds the lock; aborting",
                  file=sys.stderr)
        return 0
    purged = 0
    skipped = 0
    total_bytes_freed = 0
    try:
        for child in base.iterdir():
            if not child.is_dir():
                continue
            manifest_file = child / "clean-merged.manifest.json"
            if not manifest_file.is_file():
                # Round-4 P2-B: orphan dir (Lane W crashed before manifest write).
                # Use the dir's mtime as fallback so it doesn't sit forever.
                try:
                    mtime = child.stat().st_mtime
                except OSError:
                    skipped += 1
                    continue
                if mtime > cutoff:
                    continue
                try:
                    size = sum(f.stat().st_size for f in child.rglob("*") if f.is_file())
                except OSError:
                    size = 0
                shutil.rmtree(child, ignore_errors=True)
                total_bytes_freed += size
                write_audit(repo_root, config, {
                    "lane": "W", "action": "quarantine-purged-orphan",
                    "quarantine_path": str(child),
                })
                purged += 1
                continue
            try:
                manifest = json.loads(manifest_file.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                skipped += 1
                continue
            if not manifest.get("worktree_remove_ok"):
                # Round-4.5 self-review: a dir with manifest but
                # worktree_remove_ok=False is a stuck half-state (archive-failed,
                # remove-failed-after-archive, or a crash between worktree-remove
                # and the manifest flip). Hold for grace, then purge as cruft.
                #
                # Round-5 (Grok P1): do NOT rmtree if worktree.tar.gz exists and
                # verifies with tar -tzf — that archive is the only recovery
                # surface for the (already-removed) worktree's tracked content
                # when the branch ref has been deleted by a later sweep. Log it
                # loudly and skip; operator must delete explicitly.
                try:
                    mtime = child.stat().st_mtime
                except OSError:
                    skipped += 1
                    continue
                if mtime > cutoff:
                    continue
                archive_file = child / "worktree.tar.gz"
                # Round-5.5 polish (GPT P2 #4): if a prior run already verified
                # this archive AND recorded verified_archive_at, skip the tar
                # call entirely. The mtime-bump handles positive grace; this
                # handles --purge-quarantine 0 too.
                # Round-5.5 polish-2 (GPT P2, Claude P3-1): gate on
                # archive_file.is_file() too — if the archive was deleted/
                # corrupted externally, the flag alone must NOT pin an empty
                # dir forever. Cheap stat; preserves the no-re-tar optimization
                # for intact archives.
                already_verified = (bool(manifest.get("verified_archive_at"))
                                    and archive_file.is_file())
                if already_verified:
                    write_audit(repo_root, config, {
                        "lane": "W", "branch": manifest.get("branch"),
                        "action": "quarantine-cruft-skipped-verified-archive",
                        "quarantine_path": str(child),
                        "reason": "previously-verified archive present; skipping re-verify",
                    })
                    skipped += 1
                    continue
                if archive_file.is_file():
                    try:
                        verify = subprocess.run(["tar", "-tzf", str(archive_file)],
                                                capture_output=True, text=True, timeout=30)
                        if verify.returncode == 0:
                            # Verified archive present — refuse to delete; surface.
                            # Round-5.5 Claude F3: touch mtime on skip so we
                            # don't re-spawn tar -tzf for this dir on every run.
                            # Round-5.5 polish (GPT P2 #4): also persist a
                            # verified_archive_at manifest field so --purge-quarantine 0
                            # (where mtime-bump alone is insufficient because
                            # cutoff=now) still skips re-verification.
                            try:
                                os.utime(child, None)
                            except OSError:
                                pass
                            try:
                                manifest["verified_archive_at"] = (
                                    dt.datetime.now(dt.timezone.utc).isoformat())
                                _atomic_write_text(manifest_file, json.dumps(
                                    manifest, indent=2, sort_keys=True))
                            except (OSError, ValueError):
                                pass
                            write_audit(repo_root, config, {
                                "lane": "W", "branch": manifest.get("branch"),
                                "action": "quarantine-cruft-skipped-verified-archive",
                                "quarantine_path": str(child),
                                "reason": "worktree_remove_ok=False but verified archive present; "
                                          "operator must remove explicitly if unwanted",
                            })
                            skipped += 1
                            continue
                    except Exception as exc:
                        write_audit(repo_root, config, {
                            "lane": "W", "branch": manifest.get("branch"),
                            "action": "quarantine-cruft-skipped-archive-verify-error",
                            "quarantine_path": str(child),
                            "reason": f"archive verification failed with exception: {exc}",
                        })
                        skipped += 1
                        continue
                # No archive (archive-failed) or corrupt archive — safe to purge.
                try:
                    size = sum(f.stat().st_size for f in child.rglob("*") if f.is_file())
                except OSError:
                    size = 0
                shutil.rmtree(child, ignore_errors=True)
                total_bytes_freed += size
                write_audit(repo_root, config, {
                    "lane": "W", "branch": manifest.get("branch"),
                    "action": "quarantine-purged-incomplete",
                    "quarantine_path": str(child), "bytes_freed": size,
                    "reason": "worktree_remove_ok was False, no usable archive; "
                              "purged as cruft after grace",
                })
                purged += 1
                continue
            try:
                mtime = child.stat().st_mtime
            except OSError:
                continue
            if mtime > cutoff:
                continue
            try:
                size = sum(f.stat().st_size for f in child.rglob("*") if f.is_file())
            except OSError:
                size = 0
            shutil.rmtree(child, ignore_errors=True)
            total_bytes_freed += size
            write_audit(repo_root, config, {
                "lane": "W", "branch": manifest.get("branch"),
                "tip_sha": manifest.get("tip_sha"), "action": "quarantine-purged",
                "quarantine_path": str(child), "bytes_freed": size,
            })
            purged += 1
    finally:
        _release_lock(fd)
    if not quiet:
        print(f"[{SCRIPT_NAME}] purge: {purged} purged, {skipped} skipped "
              f"(no/failed manifest), {total_bytes_freed} bytes freed")
    return purged


def cmd_prune_backups(
    repo_root: pathlib.Path, config: Config, *, days: int, quiet: bool,
) -> int:
    """Prune backup refs older than `days`.

    Backup ref names embed the creation timestamp: refs/clean-merged/<branch>-<sha>-<unix_ts>.
    Round-4 P1 (Claude/GPT): pruning by %(committerdate:unix) used the ORIGINAL
    commit's date, not the backup's creation time — so a backup created today
    for a year-old commit was immediately prune-eligible (effective recovery
    window ~0s). We now parse the embedded creation timestamp from the ref name.
    """
    out = _git(repo_root, ["for-each-ref", "--format=%(refname)", "refs/clean-merged/"])
    cutoff = time.time() - days * 86400
    pruned = 0
    skipped_unparseable = 0
    for line in out.stdout.splitlines():
        ref = line.strip()
        if not ref:
            continue
        # Ref name ends with <safe-branch>-<short-sha>-<unix-ts>. Parse from the right.
        m = re.match(r"^(.+)-([0-9a-f]{7,40})-(\d+)$", ref[len("refs/clean-merged/"):])
        if not m:
            skipped_unparseable += 1
            continue
        try:
            created_ts = int(m.group(3))
        except ValueError:
            skipped_unparseable += 1
            continue
        if created_ts > cutoff:
            continue
        _git(repo_root, ["update-ref", "-d", ref], check=False)
        pruned += 1
    if not quiet:
        print(f"[{SCRIPT_NAME}] prune-backups: {pruned} removed (>{days}d old), "
              f"{skipped_unparseable} unparseable skipped")
    return pruned


def _is_disabled(env_value: str | None) -> bool:
    """Shared kill-switch truthiness so bash hooks and Python agree.

    Round-4 (Claude P1-5) flagged the original split-brain: bash used
    `[ -n ... ]` (any non-empty disables), Python used `== "1"` only — so
    CLEAN_MERGED_DISABLED=0 silenced hooks but enabled manual runs. Round-4's
    fix updated Python only; round-4.5 self-review caught that bash was still
    on `[ -n ]`, leaving the parity claim false. Bash hooks now use a `case`
    block matching this exact rule.

    Shared rule: empty/0/false/no/off (case-insensitive) = enabled;
    anything else = disabled.

    Round-5.5 (Kimi/Claude/Grok/GPT P2): documented contract is ASCII
    whitespace only. Python's `.strip()` would also strip Unicode whitespace
    (NBSP, em space, etc.); bash's `[[:space:]]` is locale-dependent and in
    a C/POSIX locale matches ASCII only. The realistic env-var values never
    contain Unicode whitespace; we deliberately accept the residual split
    for the rare NBSP-padded value rather than complicate the bash helper.
    Operators should set CLEAN_MERGED_DISABLED to a bare ASCII value.
    """
    if env_value is None:
        return False
    v = env_value.strip().lower()
    return v not in ("", "0", "false", "no", "off")


def cmd_doctor_on_error(repo_root: pathlib.Path, exc: Exception) -> int:
    """Doctor path that runs even when config parse failed (round-4 P1-6).

    main() catches ConfigError before the doctor dispatch and returned 0 — so
    the one failure doctor most needs to report was unsurfaced. This helper
    reports the config error and the install state, returns 1.
    """
    print(f"[{SCRIPT_NAME}] doctor")
    print(f"  CONFIG ERROR             = {exc}")
    print(f"  python                   = {sys.version_info.major}.{sys.version_info.minor} "
          f"(tomllib={'yes' if _HAS_TOMLLIB else 'fallback flat parser'})")
    try:
        common = git_common_dir(repo_root)
        print(f"  git-common-dir           = {common}")
        active = _git(repo_root, ["config", "--get", "core.hooksPath"], check=False)
        if active.returncode == 0 and active.stdout.strip():
            hooks_dir = pathlib.Path(active.stdout.strip())
            if not hooks_dir.is_absolute():
                hooks_dir = repo_root / hooks_dir
            print(f"  core.hooksPath           = {hooks_dir}")
            for h in ("post-merge", "post-checkout", "post-rewrite"):
                f = hooks_dir / h
                managed = f.is_file() and HOOK_MARKER in f.read_text(encoding="utf-8", errors="replace")
                print(f"  hook {h:14s} managed={managed}")
        # heartbeat freshness even on config error
        import datetime as _dt
        hb_paths = [
            common / "clean-merged.heartbeat",
            repo_root / ".git" / "clean-merged.heartbeat",
        ]
        for hb in hb_paths:
            if hb.is_file():
                try:
                    hb_ts = _dt.datetime.fromisoformat(hb.read_text(encoding="utf-8").strip())
                    age = _dt.datetime.now(_dt.timezone.utc) - hb_ts
                    print(f"  heartbeat age            = {age} (at {hb})")
                except (ValueError, OSError):
                    pass
                break
    except Exception as inner:  # noqa: BLE001
        print(f"  (additional error during doctor: {inner})")
    print()
    print(f"[{SCRIPT_NAME}] config parse failed; lanes are silently halted. "
          "Fix the config to resume automatic cleanup.")
    return 1


def cmd_doctor(repo_root: pathlib.Path, config: Config) -> int:
    problems: list[str] = []
    print(f"[{SCRIPT_NAME}] doctor")

    # Python / tomllib availability
    py_version = f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}"
    print(f"  python                   = {py_version} (tomllib={'yes' if _HAS_TOMLLIB else 'fallback flat parser'})")
    if not _HAS_TOMLLIB and sys.version_info < (3, 11):
        # fallback is in use; fine, but surface it
        print(f"  note                     = using fallback TOML reader (Python < 3.11)")

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

    # remote.<remote>.prune (round-4 P2: was hardcoded to origin)
    prune_key = f"remote.{config.remote_name}.prune"
    prune = _git(repo_root, ["config", "--get", prune_key], check=False)
    print(f"  {prune_key:25s} = {prune.stdout.strip() or '(unset)'}")
    if prune.stdout.strip() != "true":
        problems.append(f"{prune_key} != true (run `just setup`)")

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

    # quarantine disk usage (round-4 P2: doctor reported count, not bytes;
    # round-5.5 Claude F2: also surface pinned verified-archive cruft dirs that
    # are intentionally never auto-purged — operator needs visibility to clean
    # them up manually when they're no longer wanted.
    # round-5.5 polish (Kimi P2/P3, Claude P3-7): pinned state is intentional
    # safety behavior, not a malfunction — report as info, NOT a problem.
    # round-5.5 polish-2 (GPT P2 #2, Claude P3-2): predicate on actual archive
    # PRESENCE (worktree.tar.gz exists), not on the internal verified_archive_at
    # field. Old pinned dirs created before that field existed are still visible,
    # and a flag-only check would mis-report a dir whose archive was deleted
    # externally. Doctor does NOT re-run tar -tzf (expensive on every doctor
    # invocation); "pinned" here means "stuck dir with an archive file present,"
    # not "archive verified intact."
    q = _resolve_path(repo_root, config.lane_w.quarantine_dir)
    if q.is_dir():
        entries = [c for c in q.iterdir() if c.is_dir()]
        total_bytes = 0
        pinned = 0
        pinned_bytes = 0
        for c in entries:
            try:
                size = sum(f.stat().st_size for f in c.rglob("*") if f.is_file())
            except OSError:
                size = 0
            total_bytes += size
            mf = c / "clean-merged.manifest.json"
            if mf.is_file():
                try:
                    m = json.loads(mf.read_text(encoding="utf-8"))
                    archive_file = c / "worktree.tar.gz"
                    if not m.get("worktree_remove_ok") and archive_file.is_file():
                        pinned += 1
                        pinned_bytes += size
                except (OSError, json.JSONDecodeError):
                    pass
        print(f"  quarantine               = {len(entries)} entries, "
              f"{total_bytes / (1024 * 1024):.1f} MiB at {q}")
        if pinned:
            # INFO only — pinned state is intentional safety behavior, not a
            # problem. Don't make doctor return non-zero just because the
            # operator is keeping recovery archives.
            print(f"  quarantine pinned (info) = {pinned} archive-present dirs "
                  f"({pinned_bytes / (1024 * 1024):.1f} MiB) — kept intentionally; "
                  "remove manually when no longer needed")
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
    p.add_argument("--allow-detached-removal", action="store_true",
                   help="Lane W: override detached-HEAD worktree refusal. "
                        "DANGEROUS: reflog-only commits in the detached worktree "
                        "are NOT preserved by the archive and become unreachable "
                        "after git gc. Only use when you have verified the detached "
                        "HEAD has no exploratory commits you want to keep.")
    p.add_argument("--purge-quarantine", nargs="?", const=-1, type=int, default=None,
                   metavar="DAYS", help="purge quarantined worktrees older than DAYS")
    p.add_argument("--prune-backups", nargs="?", const=-1, type=int, default=None,
                   metavar="DAYS", help="prune backup refs older than DAYS")
    p.add_argument("--doctor", action="store_true", help="run diagnostic")
    return p


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    # Resolve to the MAIN worktree root, not the current worktree's root.
    # (round-4.5 self-review / Kimi P1: if the operator runs Lane W from inside
    # a worktree that Lane W removes, the cwd-based repo_root becomes invalid
    # mid-sweep and subsequent _git calls raise FileNotFoundError. Pin to the
    # main worktree root, which is never removed.)
    #
    # round-5 Grok P1: keep the INVOKER's root too so Lane W's active-worktree
    # skip can detect "the worktree I'm standing in" — without this, `cur` would
    # be the MAIN worktree's branch, and a Lane W run from inside a feature
    # worktree would happily archive that very worktree.
    #
    # gemini-code-assist P2: _resolve_repo_root and (transitively) load_config
    # can raise CleanMergedError (the parent of ConfigError). Catching only
    # ConfigError left a crash path if invoked outside a git repo or if the
    # git common dir couldn't be resolved. Catch the parent for graceful exit.
    try:
        invoke_root = _resolve_repo_root(pathlib.Path.cwd())
        repo_root = _main_worktree_root(invoke_root)
        config = load_config(repo_root)
    except CleanMergedError as exc:
        # Don't crash the hook chain on config or git-resolution errors.
        # Round-4 P1-6: BUT if the operator explicitly asked for --doctor,
        # surface the diagnostic and exit non-zero.
        if not args.quiet:
            print(f"[{SCRIPT_NAME}] error: {exc}", file=sys.stderr)
        if args.doctor:
            return cmd_doctor_on_error(repo_root=pathlib.Path.cwd(), exc=exc)
        return 0

    if _is_disabled(os.environ.get("CLEAN_MERGED_DISABLED")) or not config.enabled:
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
            invoke_root=invoke_root,
            allow_detached_removal=args.allow_detached_removal,
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
                invoke_root=invoke_root,
                allow_detached_removal=args.allow_detached_removal,
            )

    # audit successful mutations.
    # Lane W records are audited inline (per-mutation) because Lane W is a
    # multi-step flow (archive -> remove -> branch-delete) where a crash between
    # steps must leave a forensic trail. Lane H/R are atomic single-line mutations
    # and are audited here at the end.
    for r in all_records:
        if r.get("lane") == "W":
            continue
        if r.get("action") in ("deleted", "branch-deleted"):
            write_audit(repo_root, config, r)
        elif r.get("action") in ("delete-refused", "delete-cas-refused",
                                  "move-failed", "branch-delete-failed"):
            write_audit(repo_root, config, r)

    return 0


if __name__ == "__main__":
    sys.exit(main())
