#!/usr/bin/env python3
"""Preflight candidate PR waves before queueing them through Mergify."""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import pathlib
import re
import shlex
import subprocess
import sys
import tempfile
import tomllib
from collections.abc import Sequence


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "rust-verification.toml"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
CONFLICT_LINE_RE = re.compile(r"^\d{6} [0-9a-f]{40} [123]\t(.+)$")
PR_REF_PREFIX = "refs/pull/"
FETCH_HEAD = "FETCH_HEAD"
PROFILE_NONE = "none"
STATUS_READY = "ready"


class PreflightError(RuntimeError):
    """Raised when preflight input or repository state is invalid."""


@dataclasses.dataclass(frozen=True)
class CommandResult:
    args: tuple[str, ...]
    returncode: int
    stdout: str
    stderr: str


@dataclasses.dataclass(frozen=True)
class MergeResult:
    clean: bool
    tree: str | None
    files: tuple[str, ...]
    raw: str


@dataclasses.dataclass(frozen=True)
class PrHead:
    number: int
    sha: str


@dataclasses.dataclass(frozen=True)
class SyntheticCommit:
    commit: str
    prs: tuple[int, ...]


@dataclasses.dataclass(frozen=True)
class VerifierResult:
    command: str
    returncode: int
    stdout: str
    stderr: str

    def as_json(self) -> dict[str, object]:
        return {
            "command": self.command,
            "returncode": self.returncode,
            "stdout": self.stdout,
            "stderr": self.stderr,
        }


@dataclasses.dataclass(frozen=True)
class OutputPolicy:
    verifier_stream_max_lines: int
    verifier_stream_max_bytes: int

    def as_json(self) -> dict[str, int]:
        return {
            "verifier_stream_max_lines": self.verifier_stream_max_lines,
            "verifier_stream_max_bytes": self.verifier_stream_max_bytes,
        }


@dataclasses.dataclass(frozen=True)
class ReadinessIssue:
    code: str
    message: str

    def as_json(self) -> dict[str, str]:
        return {
            "code": self.code,
            "message": self.message,
        }


@dataclasses.dataclass(frozen=True)
class MetadataExpectation:
    code: str
    field: str
    expected: object
    message: str
    warn_when_missing: bool = True

    def evaluate(self, payload: dict[str, object]) -> ReadinessIssue | None:
        actual = payload.get(self.field)
        if actual == self.expected:
            return None
        if actual is None and not self.warn_when_missing:
            return None
        return ReadinessIssue(
            code=self.code,
            message=self.message.format(actual=actual, expected=self.expected),
        )


@dataclasses.dataclass(frozen=True)
class DynamicExpectation:
    code: str
    field: str
    expected_name: str
    message: str

    def evaluate(
        self,
        payload: dict[str, object],
        expected_values: dict[str, str | None],
    ) -> ReadinessIssue | None:
        expected = expected_values[self.expected_name]
        if expected is None:
            return None
        actual = payload.get(self.field)
        if actual == expected:
            return None
        return ReadinessIssue(
            code=self.code,
            message=self.message.format(actual=actual, expected=expected),
        )


@dataclasses.dataclass(frozen=True)
class Batch:
    index: int
    commit: str
    prs: tuple[int, ...]
    verifiers: tuple[VerifierResult, ...]

    def as_json(self) -> dict[str, object]:
        return {
            "index": self.index,
            "prs": list(self.prs),
            "status": STATUS_READY,
            "verifiers": [result.as_json() for result in self.verifiers],
        }


@dataclasses.dataclass(frozen=True)
class PreflightConfig:
    origin: str
    base: str
    default_verifier_profile: str
    verifier_profiles: dict[str, tuple[str, ...]]
    output_policy: OutputPolicy


STATIC_READINESS_EXPECTATIONS = (
    MetadataExpectation("not_open", "state", "OPEN", "PR is not open"),
    MetadataExpectation("draft", "isDraft", False, "PR is draft", warn_when_missing=False),
    MetadataExpectation(
        "not_mergeable",
        "mergeable",
        "MERGEABLE",
        "PR mergeable state is {actual}",
    ),
    MetadataExpectation(
        "review_not_approved",
        "reviewDecision",
        "APPROVED",
        "review decision is {actual}",
    ),
)
DYNAMIC_READINESS_EXPECTATIONS = (
    DynamicExpectation(
        "base_mismatch",
        "baseRefName",
        "expected_base",
        "PR targets base {actual!r}, expected {expected!r}",
    ),
    DynamicExpectation(
        "head_mismatch",
        "headRefOid",
        "fetched_head",
        "GitHub headRefOid {actual} does not match fetched PR head {expected}",
    ),
)
CHECK_BUCKET_ISSUES = {
    "fail": ("required_check_failed", "required check failed: {name}"),
    "cancel": ("required_check_failed", "required check failed: {name}"),
    "pending": ("required_check_pending", "required check pending: {name}"),
}


def run_command(
    args: Sequence[str],
    *,
    cwd: pathlib.Path,
    check: bool = True,
    env: dict[str, str] | None = None,
) -> CommandResult:
    completed = subprocess.run(
        list(args),
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        env=env,
    )
    result = CommandResult(
        args=tuple(args),
        returncode=completed.returncode,
        stdout=completed.stdout,
        stderr=completed.stderr,
    )
    if check and result.returncode != 0:
        rendered = " ".join(shlex.quote(part) for part in result.args)
        raise PreflightError(
            f"command failed ({result.returncode}): {rendered}\n{result.stderr}{result.stdout}"
        )
    return result


def git(repo: pathlib.Path, *args: str, check: bool = True) -> CommandResult:
    return run_command(["git", *args], cwd=repo, check=check)


def load_toml(path: pathlib.Path) -> dict[str, object]:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise PreflightError(f"config missing: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise PreflightError(f"config is invalid TOML: {exc}") from exc
    if not isinstance(data, dict):
        raise PreflightError("config root must be a TOML table")
    return data


def require_table(parent: dict[str, object], key: str, prefix: str) -> dict[str, object]:
    value = parent.get(key)
    if not isinstance(value, dict):
        raise PreflightError(f"{prefix}.{key} must be a table")
    return value


def require_string(parent: dict[str, object], key: str, prefix: str) -> str:
    value = parent.get(key)
    if not isinstance(value, str) or not value:
        raise PreflightError(f"{prefix}.{key} must be a non-empty string")
    return value


def require_positive_int(parent: dict[str, object], key: str, prefix: str) -> int:
    value = parent.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise PreflightError(f"{prefix}.{key} must be a positive integer")
    return value


def load_config(path: pathlib.Path) -> PreflightConfig:
    root = load_toml(path)
    settings = require_table(root, "merge_queue_preflight", "config")
    origin = require_string(settings, "origin", "config.merge_queue_preflight")
    base = require_string(settings, "base", "config.merge_queue_preflight")
    default_profile = require_string(
        settings, "default_verifier_profile", "config.merge_queue_preflight"
    )
    profiles_root = require_table(
        settings, "verifier_profiles", "config.merge_queue_preflight"
    )
    output_settings = require_table(settings, "output", "config.merge_queue_preflight")
    output_policy = OutputPolicy(
        verifier_stream_max_lines=require_positive_int(
            output_settings,
            "verifier_stream_max_lines",
            "config.merge_queue_preflight.output",
        ),
        verifier_stream_max_bytes=require_positive_int(
            output_settings,
            "verifier_stream_max_bytes",
            "config.merge_queue_preflight.output",
        ),
    )
    profiles: dict[str, tuple[str, ...]] = {}
    for profile_name, raw_profile in profiles_root.items():
        if not isinstance(raw_profile, dict):
            raise PreflightError(
                f"config.merge_queue_preflight.verifier_profiles.{profile_name} must be a table"
            )
        raw_commands = raw_profile.get("commands")
        if not isinstance(raw_commands, list) or any(
            not isinstance(command, str) or not command for command in raw_commands
        ):
            raise PreflightError(
                f"config.merge_queue_preflight.verifier_profiles.{profile_name}.commands must be a string array"
            )
        profiles[profile_name] = tuple(raw_commands)
    if default_profile not in profiles:
        raise PreflightError(
            f"config.merge_queue_preflight.default_verifier_profile {default_profile!r} has no profile"
        )
    return PreflightConfig(
        origin=origin,
        base=base,
        default_verifier_profile=default_profile,
        verifier_profiles=profiles,
        output_policy=output_policy,
    )


def positive_pr_number(value: str) -> int:
    if not value.isdecimal() or int(value) <= 0:
        raise argparse.ArgumentTypeError("PR numbers must be positive integers")
    return int(value)


def unique_preserving_order(values: Sequence[int]) -> tuple[int, ...]:
    seen: set[int] = set()
    ordered: list[int] = []
    for value in values:
        if value in seen:
            raise PreflightError(f"PR #{value} was provided more than once")
        seen.add(value)
        ordered.append(value)
    return tuple(ordered)


def fetch_base(repo: pathlib.Path, origin: str, base: str) -> str:
    git(repo, "fetch", "--quiet", origin, base)
    sha = git(repo, "rev-parse", FETCH_HEAD).stdout.strip()
    if SHA_RE.fullmatch(sha) is None:
        raise PreflightError(f"base {base!r} did not resolve to a commit SHA")
    return sha


def fetch_pr_head(repo: pathlib.Path, origin: str, pr_number: int) -> PrHead:
    git(repo, "fetch", "--quiet", origin, f"{PR_REF_PREFIX}{pr_number}/head")
    sha = git(repo, "rev-parse", FETCH_HEAD).stdout.strip()
    if SHA_RE.fullmatch(sha) is None:
        raise PreflightError(f"PR #{pr_number} did not resolve to a commit SHA")
    return PrHead(number=pr_number, sha=sha)


def parse_conflict_files(output: str) -> tuple[str, ...]:
    files: set[str] = set()
    for line in output.splitlines():
        match = CONFLICT_LINE_RE.match(line)
        if match is not None:
            files.add(match.group(1))
    if files:
        return tuple(sorted(files))
    fallback: set[str] = set()
    for line in output.splitlines():
        if line.startswith("CONFLICT ") and " in " in line:
            fallback.add(line.rsplit(" in ", 1)[1])
    return tuple(sorted(fallback))


def merge_tree(repo: pathlib.Path, left: str, right: str) -> MergeResult:
    result = git(repo, "merge-tree", "--write-tree", left, right, check=False)
    output = result.stdout + result.stderr
    if result.returncode == 0:
        tree = result.stdout.splitlines()[0].strip()
        if SHA_RE.fullmatch(tree) is None:
            raise PreflightError("git merge-tree returned an invalid tree SHA")
        return MergeResult(clean=True, tree=tree, files=(), raw=output)
    return MergeResult(
        clean=False,
        tree=None,
        files=parse_conflict_files(output),
        raw=output,
    )


def commit_tree(repo: pathlib.Path, tree: str, parents: Sequence[str], message: str) -> str:
    args = ["commit-tree", tree]
    for parent in parents:
        args.extend(["-p", parent])
    env = os.environ.copy()
    env.setdefault("GIT_AUTHOR_NAME", "merge-queue-preflight")
    env.setdefault("GIT_AUTHOR_EMAIL", "merge-queue-preflight@example.invalid")
    env.setdefault("GIT_COMMITTER_NAME", "merge-queue-preflight")
    env.setdefault("GIT_COMMITTER_EMAIL", "merge-queue-preflight@example.invalid")
    completed = subprocess.run(
        ["git", *args],
        cwd=repo,
        input=message,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        env=env,
    )
    if completed.returncode != 0:
        raise PreflightError(f"git commit-tree failed: {completed.stderr}{completed.stdout}")
    sha = completed.stdout.strip()
    if SHA_RE.fullmatch(sha) is None:
        raise PreflightError("git commit-tree returned an invalid commit SHA")
    return sha


def synthesize_merge(
    repo: pathlib.Path,
    left_commit: str,
    right_commit: str,
    prs: Sequence[int],
) -> SyntheticCommit | MergeResult:
    merged = merge_tree(repo, left_commit, right_commit)
    if not merged.clean or merged.tree is None:
        return merged
    message = "merge queue preflight: " + ",".join(f"#{pr}" for pr in prs)
    commit = commit_tree(repo, merged.tree, [left_commit, right_commit], message)
    return SyntheticCommit(commit=commit, prs=tuple(prs))


def run_verifier_commands(
    repo: pathlib.Path,
    commit: str,
    commands: Sequence[str],
) -> tuple[VerifierResult, ...]:
    if not commands:
        return ()
    results: list[VerifierResult] = []
    with tempfile.TemporaryDirectory(prefix="merge-queue-preflight-") as tmp:
        worktree = pathlib.Path(tmp) / "worktree"
        git(repo, "worktree", "add", "--quiet", "--detach", str(worktree), commit)
        try:
            for command in commands:
                parts = shlex.split(command)
                if not parts:
                    raise PreflightError("verifier command must not be empty")
                completed = run_command(parts, cwd=worktree, check=False)
                verifier_result = VerifierResult(
                    command=command,
                    returncode=completed.returncode,
                    stdout=completed.stdout,
                    stderr=completed.stderr,
                )
                results.append(verifier_result)
                if verifier_result.returncode != 0:
                    break
        finally:
            git(repo, "worktree", "remove", "--force", str(worktree), check=False)
    return tuple(results)


def first_failed_verifier(results: Sequence[VerifierResult]) -> VerifierResult | None:
    for result in results:
        if result.returncode != 0:
            return result
    return None


def verifier_block(pr: int, result: VerifierResult) -> dict[str, object]:
    return {
        "pr": pr,
        "reason": f"verifier failed: {result.command}",
        "type": "verifier_failed",
        **result.as_json(),
    }


def gh_json(args: Sequence[str]) -> object:
    try:
        completed = subprocess.run(
            ["gh", *args],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except FileNotFoundError as exc:
        raise PreflightError("gh executable not found") from exc
    if completed.returncode not in {0, 8}:
        raise PreflightError(f"gh {' '.join(args)} failed: {completed.stderr}{completed.stdout}")
    try:
        return json.loads(completed.stdout or "[]")
    except json.JSONDecodeError as exc:
        raise PreflightError(f"gh {' '.join(args)} returned invalid JSON") from exc


def readiness_issues(
    payload: dict[str, object],
    checks: Sequence[object],
    *,
    expected_base: str | None,
    fetched_head: str | None,
) -> tuple[ReadinessIssue, ...]:
    expected_values = {
        "expected_base": expected_base,
        "fetched_head": fetched_head,
    }
    issues = [
        issue
        for rule in STATIC_READINESS_EXPECTATIONS
        if (issue := rule.evaluate(payload)) is not None
    ]
    issues.extend(
        issue
        for rule in DYNAMIC_READINESS_EXPECTATIONS
        if (issue := rule.evaluate(payload, expected_values)) is not None
    )
    for check in checks:
        if not isinstance(check, dict):
            continue
        bucket = check.get("bucket")
        issue_template = CHECK_BUCKET_ISSUES.get(bucket)
        if issue_template is None:
            continue
        code, message = issue_template
        issues.append(
            ReadinessIssue(
                code=code,
                message=message.format(name=check.get("name")),
            )
        )
    return tuple(issues)


def pr_readiness(
    pr_number: int,
    *,
    use_gh: bool,
    expected_base: str | None = None,
    fetched_head: str | None = None,
) -> dict[str, object]:
    if not use_gh:
        return {"pr": pr_number, "warnings": [], "warning_details": [], "checks": []}
    payload = gh_json(
        [
            "pr",
            "view",
            str(pr_number),
            "--json",
            "number,state,isDraft,mergeable,reviewDecision,headRefOid,baseRefName,title,url",
        ]
    )
    if not isinstance(payload, dict):
        raise PreflightError(f"gh pr view {pr_number} did not return an object")
    checks = gh_json(
        [
            "pr",
            "checks",
            str(pr_number),
            "--required",
            "--json",
            "name,state,bucket,workflow",
        ]
    )
    if not isinstance(checks, list):
        raise PreflightError(f"gh pr checks {pr_number} did not return a list")
    issues = readiness_issues(
        payload,
        checks,
        expected_base=expected_base,
        fetched_head=fetched_head,
    )
    return {
        "pr": pr_number,
        "warnings": [issue.message for issue in issues],
        "warning_details": [issue.as_json() for issue in issues],
        "metadata": payload,
        "checks": checks,
    }


def readiness_for_wave(
    pr_numbers: Sequence[int],
    *,
    use_gh: bool,
    base: str,
    heads: dict[int, PrHead],
) -> tuple[list[dict[str, object]], list[str]]:
    if not use_gh:
        return [pr_readiness(pr, use_gh=False) for pr in pr_numbers], []
    readiness: list[dict[str, object]] = []
    metadata_warnings: list[str] = []
    for pr in pr_numbers:
        try:
            readiness.append(
                pr_readiness(
                    pr,
                    use_gh=True,
                    expected_base=base,
                    fetched_head=heads[pr].sha,
                )
            )
        except PreflightError as exc:
            warning = f"GitHub metadata unavailable for PR #{pr}; readiness checks skipped: {exc}"
            metadata_warnings.append(warning)
            readiness.append(
                {
                    "pr": pr,
                    "warnings": [],
                    "warning_details": [],
                    "checks": [],
                    "metadata_unavailable": True,
                    "metadata_error": str(exc),
                }
            )
    return readiness, metadata_warnings


def preflight(
    *,
    repo: pathlib.Path,
    origin: str,
    base: str,
    pr_numbers: Sequence[int],
    verifier_commands: Sequence[str],
    output_policy: OutputPolicy,
    use_gh: bool,
) -> tuple[dict[str, object], int]:
    requested = unique_preserving_order(pr_numbers)
    base_sha = fetch_base(repo, origin, base)
    heads = {head.number: head for head in (fetch_pr_head(repo, origin, pr) for pr in requested)}
    readiness, metadata_warnings = readiness_for_wave(
        requested,
        use_gh=use_gh,
        base=base,
        heads=heads,
    )
    readiness_blocks = [
        {
            "pr": item["pr"],
            "reason": "; ".join(item["warnings"]),
            "type": "readiness_failed",
        }
        for item in readiness
        if item["warnings"]
    ]
    blocked_prs: list[dict[str, object]] = list(readiness_blocks)
    blocked_numbers = {int(block["pr"]) for block in blocked_prs}
    base_commits: dict[int, SyntheticCommit] = {}
    base_verifiers: dict[int, tuple[VerifierResult, ...]] = {}
    for pr in requested:
        if pr in blocked_numbers:
            continue
        head = heads[pr]
        synthetic = synthesize_merge(repo, base_sha, head.sha, [pr])
        if isinstance(synthetic, MergeResult):
            blocked_prs.append(
                {
                    "pr": pr,
                    "reason": "conflicts with base",
                    "files": list(synthetic.files),
                    "type": "base_conflict",
                }
            )
            blocked_numbers.add(pr)
            continue
        verifier_results = run_verifier_commands(repo, synthetic.commit, verifier_commands)
        failed = first_failed_verifier(verifier_results)
        if failed is not None:
            blocked_prs.append(verifier_block(pr, failed))
            blocked_numbers.add(pr)
            continue
        base_commits[pr] = synthetic
        base_verifiers[pr] = verifier_results

    conflicts: list[dict[str, object]] = []
    batches: list[Batch] = []
    current: SyntheticCommit | None = None
    current_verifiers: tuple[VerifierResult, ...] = ()
    batch_index = 1
    for pr in requested:
        if pr in blocked_numbers:
            continue
        pr_head = heads[pr]
        if current is None:
            current = base_commits[pr]
            current_verifiers = base_verifiers[pr]
            continue
        candidate_prs = [*current.prs, pr]
        synthetic = synthesize_merge(repo, current.commit, pr_head.sha, candidate_prs)
        if isinstance(synthetic, MergeResult):
            conflicts.append(
                {
                    "pr": pr,
                    "against_batch": list(current.prs),
                    "files": list(synthetic.files),
                    "type": "batch_conflict",
                }
            )
            batches.append(
                Batch(
                    index=batch_index,
                    commit=current.commit,
                    prs=current.prs,
                    verifiers=current_verifiers,
                )
            )
            batch_index += 1
            current = base_commits[pr]
            current_verifiers = base_verifiers[pr]
            continue
        candidate_verifiers = run_verifier_commands(repo, synthetic.commit, verifier_commands)
        failed = first_failed_verifier(candidate_verifiers)
        if failed is not None:
            conflicts.append(
                {
                    "pr": pr,
                    "against_batch": list(current.prs),
                    "type": "batch_verifier_failed",
                    **failed.as_json(),
                }
            )
            batches.append(
                Batch(
                    index=batch_index,
                    commit=current.commit,
                    prs=current.prs,
                    verifiers=current_verifiers,
                )
            )
            batch_index += 1
            current = base_commits[pr]
            current_verifiers = base_verifiers[pr]
            continue
        current = synthetic
        current_verifiers = candidate_verifiers
    if current is not None:
        batches.append(
            Batch(
                index=batch_index,
                commit=current.commit,
                prs=current.prs,
                verifiers=current_verifiers,
            )
        )
    payload = {
        "base": base,
        "base_sha": base_sha,
        "requested_prs": list(requested),
        "pr_heads": {str(number): head.sha for number, head in heads.items()},
        "readiness": readiness,
        "metadata_warnings": metadata_warnings,
        "batches": [batch.as_json() for batch in batches],
        "blocked_prs": blocked_prs,
        "conflicts": conflicts,
        "output_policy": output_policy.as_json(),
    }
    exit_code = 1 if blocked_prs or conflicts or metadata_warnings else 0
    return payload, exit_code


def output_policy_from_payload(payload: dict[str, object]) -> OutputPolicy:
    value = payload["output_policy"]
    if not isinstance(value, dict):
        raise PreflightError("payload output_policy must be an object")
    return OutputPolicy(
        verifier_stream_max_lines=int(value["verifier_stream_max_lines"]),
        verifier_stream_max_bytes=int(value["verifier_stream_max_bytes"]),
    )


def bounded_stream_lines(output: str, output_policy: OutputPolicy) -> tuple[list[str], bool]:
    encoded = output.encode("utf-8")
    byte_truncated = len(encoded) > output_policy.verifier_stream_max_bytes
    if byte_truncated:
        output = encoded[: output_policy.verifier_stream_max_bytes].decode(
            "utf-8",
            errors="ignore",
        )
    stream_lines = output.rstrip().splitlines()
    line_truncated = len(stream_lines) > output_policy.verifier_stream_max_lines
    return stream_lines[: output_policy.verifier_stream_max_lines], byte_truncated or line_truncated


def append_verifier_result(
    lines: list[str],
    verifier: dict[str, object],
    *,
    indent: str,
    output_policy: OutputPolicy,
) -> None:
    lines.append(
        "{indent}verifier {command}: exit {returncode}".format(
            indent=indent,
            command=verifier["command"],
            returncode=verifier["returncode"],
        )
    )
    if verifier["returncode"] == 0:
        return
    for stream in ("stdout", "stderr"):
        output = str(verifier.get(stream, ""))
        if not output:
            continue
        lines.append(f"{indent}  {stream}:")
        stream_lines, truncated = bounded_stream_lines(output, output_policy)
        lines.extend(f"{indent}    {line}" for line in stream_lines)
        if truncated:
            lines.append(f"{indent}    ... truncated by merge_queue_preflight output policy")


def plain_text(payload: dict[str, object]) -> str:
    output_policy = output_policy_from_payload(payload)
    lines = [
        f"base: {payload['base']} {payload['base_sha']}",
        "requested PRs: " + ", ".join(f"#{pr}" for pr in payload["requested_prs"]),
        "recommended batches:",
    ]
    for batch in payload["batches"]:
        lines.append("  batch {index}: {prs}".format(
            index=batch["index"],
            prs=", ".join(f"#{pr}" for pr in batch["prs"]),
        ))
        for verifier in batch["verifiers"]:
            append_verifier_result(
                lines,
                verifier,
                indent="    ",
                output_policy=output_policy,
            )
    if payload["blocked_prs"]:
        lines.append("blocked PRs:")
        for item in payload["blocked_prs"]:
            lines.append(f"  #{item['pr']}: {item['reason']}")
            if item.get("files"):
                lines.append("    files: " + ", ".join(item["files"]))
            if "command" in item:
                append_verifier_result(
                    lines,
                    item,
                    indent="    ",
                    output_policy=output_policy,
                )
    if payload["metadata_warnings"]:
        lines.append("metadata warnings:")
        for warning in payload["metadata_warnings"]:
            lines.append(f"  {warning}")
    if payload["conflicts"]:
        lines.append("conflicts:")
        for item in payload["conflicts"]:
            context = ", ".join(f"#{pr}" for pr in item.get("against_batch", []))
            lines.append(f"  #{item['pr']} vs [{context}]: {item['type']}")
            if item.get("files"):
                lines.append("    files: " + ", ".join(item["files"]))
            if "command" in item:
                append_verifier_result(
                    lines,
                    item,
                    indent="    ",
                    output_policy=output_policy,
                )
    warnings = [
        (item["pr"], warning)
        for item in payload["readiness"]
        for warning in item.get("warnings", [])
    ]
    if warnings:
        lines.append("readiness warnings:")
        for pr, warning in warnings:
            lines.append(f"  #{pr}: {warning}")
    return "\n".join(lines)


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(prog="merge_queue_preflight.py")
    root.add_argument("prs", nargs="+", type=positive_pr_number)
    root.add_argument("--base")
    root.add_argument("--origin")
    root.add_argument("--config", type=pathlib.Path, default=DEFAULT_CONFIG)
    root.add_argument("--verifier-profile")
    root.add_argument("--run-verifier", action="append", default=[])
    root.add_argument("--no-gh", action="store_true")
    root.add_argument("--json", action="store_true")
    return root


def verifier_commands(config: PreflightConfig, profile: str | None, extra: Sequence[str]) -> tuple[str, ...]:
    selected = profile or config.default_verifier_profile
    if selected not in config.verifier_profiles:
        raise PreflightError(f"unknown verifier profile {selected!r}")
    return (*config.verifier_profiles[selected], *extra)


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        config = load_config(args.config)
        payload, exit_code = preflight(
            repo=pathlib.Path.cwd(),
            origin=args.origin or config.origin,
            base=args.base or config.base,
            pr_numbers=args.prs,
            verifier_commands=verifier_commands(config, args.verifier_profile, args.run_verifier),
            output_policy=config.output_policy,
            use_gh=not args.no_gh,
        )
    except PreflightError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(plain_text(payload))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
