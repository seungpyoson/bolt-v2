#!/usr/bin/env python3
"""Self-authorizing governance diff analyzer relocated from verify_ci_workflow_hygiene."""

from __future__ import annotations

import difflib
import pathlib
import re
import subprocess
from typing import NamedTuple

from workflow_expression_analysis import YAML_KEY_PATTERN, strip_comment, unquote_yaml_scalar

SELF_AUTHORIZING_GOVERNANCE_PATHS = (
    "AGENTS.md",
    ".specify/memory/constitution.md",
    ".pr_agent.toml",
    "ci/ai-review.toml",
)
SELF_AUTHORIZING_GITHUB_AUTOMATION_PREFIXES = (
    ".github/workflows/",
    ".github/actions/",
)
SELF_AUTHORIZING_ALLOWLIST_ENTRY_PATHS = (
    "ci/bolt-v3-boundary-exemptions.toml",
    "ci/doc-decoupling-residuals.toml",
    "specs/711-capital-admission-rename/misnomer-allowlist.txt",
)
SELF_AUTHORIZING_CAPABILITY_PATHS = (
    *SELF_AUTHORIZING_GITHUB_AUTOMATION_PREFIXES,
    *SELF_AUTHORIZING_ALLOWLIST_ENTRY_PATHS,
)
SELF_AUTHORIZING_SECRET_REF_RE = re.compile(
    r"""\bsecrets\s*(?:\.\s*([A-Za-z_][A-Za-z0-9_]*)\b"""
    r"""|\[\s*'([A-Za-z_][A-Za-z0-9_]*)'\s*\]"""
    r"""|\[\s*"([A-Za-z_][A-Za-z0-9_]*)"\s*\]"""
    r"""|\[\s*([^\]\s][^\]]*?)\s*\])"""
)
SELF_AUTHORIZING_SECRETS_INHERIT_RE = re.compile(
    rf"^\s*({YAML_KEY_PATTERN})\s*:\s*({YAML_KEY_PATTERN})\s*$"
)
class SelfAuthorizingCapabilitySignal(NamedTuple):
    kind: str
    detail: str
    path: str


class SelfAuthorizingDiffError(Exception):
    pass


def repo_git_bytes(repo: pathlib.Path, args: list[str]) -> bytes:
    try:
        completed = subprocess.run(
            ["git", *args],
            cwd=repo,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except subprocess.CalledProcessError as exc:
        stderr = exc.stderr.decode("utf-8", errors="replace").strip()
        stdout = exc.stdout.decode("utf-8", errors="replace").strip()
        detail = stderr or stdout or f"exit {exc.returncode}"
        raise SelfAuthorizingDiffError(f"git {' '.join(args)} failed: {detail}") from exc
    return completed.stdout


def repo_git_text_at_ref(repo: pathlib.Path, ref: str, relative_path: str) -> str:
    completed = subprocess.run(
        ["git", "show", f"{ref}:{relative_path}"],
        cwd=repo,
        encoding="utf-8",
        errors="surrogateescape",
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        return ""
    return completed.stdout


def self_authorizing_changed_paths(
    repo: pathlib.Path,
    base_ref: str,
    head_ref: str,
    pathspecs: tuple[str, ...],
) -> list[str]:
    output = repo_git_bytes(
        repo,
        [
            "-c",
            "core.quotePath=false",
            "diff",
            "--name-only",
            "-z",
            f"{base_ref}...{head_ref}",
            "--",
            *pathspecs,
        ],
    )
    return [
        path.decode("utf-8", errors="surrogateescape")
        for path in output.split(b"\0")
        if path
    ]


def self_authorizing_added_lines(
    repo: pathlib.Path,
    base_ref: str,
    head_ref: str,
    pathspecs: tuple[str, ...],
) -> list[tuple[str, str]]:
    added: list[tuple[str, str]] = []
    for relative_path in self_authorizing_changed_paths(repo, base_ref, head_ref, pathspecs):
        head_text = repo_git_text_at_ref(repo, head_ref, relative_path)
        if not head_text:
            continue
        base_lines = repo_git_text_at_ref(repo, base_ref, relative_path).splitlines()
        head_lines = head_text.splitlines()
        matcher = difflib.SequenceMatcher(
            None,
            base_lines,
            head_lines,
            autojunk=False,
        )
        for tag, _base_start, _base_end, head_start, head_end in matcher.get_opcodes():
            if tag in {"insert", "replace"}:
                added.extend((relative_path, line) for line in head_lines[head_start:head_end])
    return added


def is_github_automation_path(relative_path: str) -> bool:
    return relative_path.endswith((".yml", ".yaml")) and relative_path.startswith(
        SELF_AUTHORIZING_GITHUB_AUTOMATION_PREFIXES
    )


def non_comment_line(text: str) -> bool:
    stripped = text.strip()
    return bool(stripped) and not stripped.startswith("#")


def self_authorizing_secret_ref_detail(match: re.Match[str]) -> str:
    dot_name, single_quoted_name, double_quoted_name, dynamic_index = match.groups()
    for secret_name in (dot_name, single_quoted_name, double_quoted_name):
        if secret_name:
            return f"secrets.{secret_name}"
    if dynamic_index:
        return f"secrets[{dynamic_index.strip()}]"
    raise AssertionError("secret reference regex matched without a secret name")


def self_authorizing_secret_ref_signals(
    added_lines: list[tuple[str, str]],
) -> list[SelfAuthorizingCapabilitySignal]:
    signals: list[SelfAuthorizingCapabilitySignal] = []
    seen: set[tuple[str, str]] = set()
    for relative_path, line in added_lines:
        if not is_github_automation_path(relative_path) or not non_comment_line(line):
            continue
        clean = strip_comment(line).rstrip()
        for match in SELF_AUTHORIZING_SECRET_REF_RE.finditer(clean):
            detail = self_authorizing_secret_ref_detail(match)
            key = (relative_path, detail)
            if key in seen:
                continue
            seen.add(key)
            signals.append(
                SelfAuthorizingCapabilitySignal(
                    "secret reference",
                    detail,
                    relative_path,
                )
            )
    return signals


def self_authorizing_secret_inherit_signals(
    added_lines: list[tuple[str, str]],
) -> list[SelfAuthorizingCapabilitySignal]:
    signals: list[SelfAuthorizingCapabilitySignal] = []
    seen: set[str] = set()
    for relative_path, line in added_lines:
        if not is_github_automation_path(relative_path) or not non_comment_line(line):
            continue
        match = SELF_AUTHORIZING_SECRETS_INHERIT_RE.match(strip_comment(line).rstrip())
        if (
            match is None
            or unquote_yaml_scalar(match.group(1)) != "secrets"
            or unquote_yaml_scalar(match.group(2)).strip() != "inherit"
        ):
            continue
        if relative_path in seen:
            continue
        seen.add(relative_path)
        signals.append(
            SelfAuthorizingCapabilitySignal(
                "secret inheritance",
                "secrets: inherit",
                relative_path,
            )
        )
    return signals


def yaml_permissions_block_exists(
    lines: list[str],
    index: int,
    parent_indent: int,
    scalar: str,
) -> bool:
    if scalar:
        return scalar not in {"null", "~"}
    for child in lines[index + 1 :]:
        child_clean = strip_comment(child).rstrip()
        if not child_clean.strip():
            continue
        child_indent = len(child_clean) - len(child_clean.lstrip(" "))
        if child_indent <= parent_indent:
            break
        return True
    return False


def yaml_permissions_block_scopes(workflow_text: str) -> set[str]:
    scopes: set[str] = set()
    stack: list[tuple[int, str]] = []
    lines = workflow_text.splitlines()
    for index, line in enumerate(lines):
        clean = strip_comment(line).rstrip()
        match = re.match(rf"^(\s*)({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if match is None:
            continue
        indent = len(match.group(1))
        key = unquote_yaml_scalar(match.group(2))
        while stack and stack[-1][0] >= indent:
            stack.pop()
        scalar = unquote_yaml_scalar(match.group(3)).strip()
        if key == "permissions" and yaml_permissions_block_exists(
            lines,
            index,
            indent,
            scalar,
        ):
            scopes.add(".".join([*(ancestor for _indent, ancestor in stack), key]))
        stack.append((indent, key))
    return scopes


def yaml_flow_mapping_grants(scalar: str) -> set[tuple[str, str]]:
    if not scalar.startswith("{") or not scalar.endswith("}"):
        return set()
    grants: set[tuple[str, str]] = set()
    inner = scalar.removeprefix("{").removesuffix("}").strip()
    if not inner:
        return grants
    for item in inner.split(","):
        if ":" not in item:
            continue
        key, value = item.split(":", 1)
        key = unquote_yaml_scalar(key.strip())
        value = unquote_yaml_scalar(value.strip())
        if key and value and value != "none":
            grants.add((key, value))
    return grants


def yaml_permissions_block_grants(
    lines: list[str],
    index: int,
    parent_indent: int,
    scalar: str,
) -> set[tuple[str, str]]:
    if scalar:
        if scalar.startswith("{") and scalar.endswith("}"):
            return yaml_flow_mapping_grants(scalar)
        if scalar not in {"{}", "none", "null", "~"}:
            return {("permissions", scalar)}
        return set()
    grants: set[tuple[str, str]] = set()
    for child in lines[index + 1 :]:
        child_clean = strip_comment(child).rstrip()
        if not child_clean.strip():
            continue
        child_indent = len(child_clean) - len(child_clean.lstrip(" "))
        if child_indent <= parent_indent:
            break
        child_match = re.match(
            rf"^\s*({YAML_KEY_PATTERN})\s*:\s*({YAML_KEY_PATTERN})\s*$",
            child_clean,
        )
        if child_match is None:
            continue
        key = unquote_yaml_scalar(child_match.group(1))
        value = unquote_yaml_scalar(child_match.group(2))
        if value != "none":
            grants.add((key, value))
    return grants


def yaml_permissions_scoped_grants(workflow_text: str) -> set[tuple[str, str, str]]:
    scoped_grants: set[tuple[str, str, str]] = set()
    stack: list[tuple[int, str]] = []
    lines = workflow_text.splitlines()
    for index, line in enumerate(lines):
        clean = strip_comment(line).rstrip()
        match = re.match(rf"^(\s*)({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if match is None:
            continue
        indent = len(match.group(1))
        key = unquote_yaml_scalar(match.group(2))
        while stack and stack[-1][0] >= indent:
            stack.pop()
        scalar = unquote_yaml_scalar(match.group(3)).strip()
        if key == "permissions":
            ancestors = [ancestor for _indent, ancestor in stack]
            scope = ".".join(ancestors) if ancestors else key
            for grant_key, grant_value in yaml_permissions_block_grants(
                lines,
                index,
                indent,
                scalar,
            ):
                scoped_grants.add((scope, grant_key, grant_value))
        stack.append((indent, key))
    return scoped_grants


def yaml_permissions_grants(workflow_text: str) -> set[tuple[str, str]]:
    return {
        (key, value)
        for _scope, key, value in yaml_permissions_scoped_grants(workflow_text)
    }


def self_authorizing_permission_grant_signals(
    base_text: str,
    head_text: str,
    relative_path: str,
) -> list[SelfAuthorizingCapabilitySignal]:
    signals: list[SelfAuthorizingCapabilitySignal] = []
    base_grants = yaml_permissions_grants(base_text)
    head_grants = yaml_permissions_grants(head_text)
    global_added = head_grants - base_grants
    for key, value in sorted(global_added):
        signals.append(
            SelfAuthorizingCapabilitySignal(
                "permissions grant",
                f"{key}: {value}",
                relative_path,
            )
        )
    base_scoped_grants = yaml_permissions_scoped_grants(base_text)
    head_scoped_grants = yaml_permissions_scoped_grants(head_text)
    for scope, key, value in sorted(head_scoped_grants - base_scoped_grants):
        if (key, value) in global_added:
            continue
        signals.append(
            SelfAuthorizingCapabilitySignal(
                "permissions grant",
                f"{scope} {key}: {value}",
                relative_path,
            )
        )
    return signals


def self_authorizing_permission_signals(
    repo: pathlib.Path,
    base_ref: str,
    head_ref: str,
    changed_paths: list[str],
) -> list[SelfAuthorizingCapabilitySignal]:
    signals: list[SelfAuthorizingCapabilitySignal] = []
    for relative_path in changed_paths:
        if not is_github_automation_path(relative_path):
            continue
        base_text = repo_git_text_at_ref(repo, base_ref, relative_path)
        head_text = repo_git_text_at_ref(repo, head_ref, relative_path)
        if not head_text:
            continue
        if yaml_permissions_block_scopes(base_text) - yaml_permissions_block_scopes(head_text):
            signals.append(
                SelfAuthorizingCapabilitySignal(
                    "permissions grant",
                    "inherited default",
                    relative_path,
                )
            )
        signals.extend(
            self_authorizing_permission_grant_signals(
                base_text,
                head_text,
                relative_path,
            )
        )
    return signals


def self_authorizing_allowlist_signals(
    added_lines: list[tuple[str, str]],
) -> list[SelfAuthorizingCapabilitySignal]:
    signals: list[SelfAuthorizingCapabilitySignal] = []
    for relative_path, line in added_lines:
        if relative_path not in SELF_AUTHORIZING_ALLOWLIST_ENTRY_PATHS:
            continue
        if not non_comment_line(line):
            continue
        signals.append(
            SelfAuthorizingCapabilitySignal(
                "allowlist/exemption entry",
                line.strip(),
                relative_path,
            )
        )
    return signals


def self_authorizing_new_active_secret_signals(
    repo: pathlib.Path,
    base_ref: str,
    head_ref: str,
    changed_paths: list[str],
) -> list[SelfAuthorizingCapabilitySignal]:
    signals: list[SelfAuthorizingCapabilitySignal] = []
    for relative_path in changed_paths:
        if not is_github_automation_path(relative_path):
            continue
        if repo_git_text_at_ref(repo, base_ref, relative_path):
            continue
        head_text = repo_git_text_at_ref(repo, head_ref, relative_path)
        if not head_text:
            continue
        head_lines = [(relative_path, line) for line in head_text.splitlines()]
        signals.extend(self_authorizing_secret_ref_signals(head_lines))
        signals.extend(self_authorizing_secret_inherit_signals(head_lines))
    return signals


def dedupe_self_authorizing_signals(
    signals: list[SelfAuthorizingCapabilitySignal],
) -> list[SelfAuthorizingCapabilitySignal]:
    unique: list[SelfAuthorizingCapabilitySignal] = []
    seen: set[tuple[str, str, str]] = set()
    for signal in signals:
        key = (signal.kind, signal.detail, signal.path)
        if key in seen:
            continue
        seen.add(key)
        unique.append(signal)
    return unique


def self_authorizing_capability_signals(
    repo: pathlib.Path,
    base_ref: str,
    head_ref: str,
) -> list[SelfAuthorizingCapabilitySignal]:
    changed_paths = self_authorizing_changed_paths(
        repo,
        base_ref,
        head_ref,
        SELF_AUTHORIZING_CAPABILITY_PATHS,
    )
    added_lines = self_authorizing_added_lines(
        repo,
        base_ref,
        head_ref,
        SELF_AUTHORIZING_CAPABILITY_PATHS,
    )
    return dedupe_self_authorizing_signals(
        [
            *self_authorizing_secret_ref_signals(added_lines),
            *self_authorizing_secret_inherit_signals(added_lines),
            *self_authorizing_new_active_secret_signals(
                repo,
                base_ref,
                head_ref,
                changed_paths,
            ),
            *self_authorizing_permission_signals(repo, base_ref, head_ref, changed_paths),
            *self_authorizing_allowlist_signals(added_lines),
        ]
    )


def self_authorizing_governance_diff_errors(
    repo: pathlib.Path,
    base_ref: str,
    head_ref: str,
) -> list[str]:
    if not base_ref or not head_ref:
        return ["self-authorizing governance detector missing PR diff context"]
    try:
        governance_changes = self_authorizing_changed_paths(
            repo,
            base_ref,
            head_ref,
            SELF_AUTHORIZING_GOVERNANCE_PATHS,
        )
    except SelfAuthorizingDiffError as exc:
        return [f"self-authorizing governance detector could not inspect PR diff: {exc}"]
    if not governance_changes:
        return []
    try:
        signals = self_authorizing_capability_signals(repo, base_ref, head_ref)
    except SelfAuthorizingDiffError as exc:
        return [f"self-authorizing governance detector could not inspect capability diff: {exc}"]
    if not signals:
        return []
    governance_summary = ", ".join(sorted(governance_changes))
    signal_summary = "; ".join(
        f"{signal.kind} {signal.detail} in {signal.path}" for signal in signals
    )
    return [
        "self-authorizing governance edit blocked: "
        f"this diff edits governance rule-files ({governance_summary}) and introduces "
        f"capability signals ({signal_summary}) in the same PR. "
        "Resolution: split this into two PRs: land the governance rule change by itself "
        "first, then open a separate capability PR after that rule is on the base branch."
    ]
