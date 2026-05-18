#!/usr/bin/env python3
"""No-mistakes gate that relies on exact-head GitHub CI, not local Cargo."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


ACCEPTED_WORKFLOWS = ("CI", "CI docs pass stub")
VALID_KINDS = ("test", "lint", "format")


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: str
    stderr: str


def _run(argv: list[str], cwd: Path) -> CommandResult:
    proc = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, check=False)
    return CommandResult(proc.returncode, proc.stdout.strip(), proc.stderr.strip())


def _required_output(argv: list[str], cwd: Path, description: str) -> str:
    result = _run(argv, cwd)
    if result.returncode != 0:
        detail = result.stderr or result.stdout or f"exit {result.returncode}"
        raise RuntimeError(f"{description} failed: {detail}")
    return result.stdout


def _json_output(argv: list[str], cwd: Path, description: str) -> object:
    raw = _required_output(argv, cwd, description)
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"{description} returned invalid JSON") from exc


def _current_head(cwd: Path) -> str:
    return _required_output(["git", "rev-parse", "HEAD"], cwd, "resolve current HEAD")


def _current_pr(cwd: Path) -> dict[str, object]:
    payload = _json_output(
        ["gh", "pr", "view", "--json", "number,headRefOid,url"],
        cwd,
        "resolve current branch pull request",
    )
    if not isinstance(payload, dict):
        raise RuntimeError("current branch pull request payload was not an object")
    return payload


def _workflow_runs(cwd: Path, head: str) -> list[dict[str, object]]:
    payload = _json_output(
        [
            "gh",
            "run",
            "list",
            "--commit",
            head,
            "--json",
            "databaseId,name,status,conclusion,headSha,url",
            "--limit",
            "50",
        ],
        cwd,
        "list GitHub workflow runs for current HEAD",
    )
    if not isinstance(payload, list):
        raise RuntimeError("workflow run payload was not a list")
    runs: list[dict[str, object]] = []
    for item in payload:
        if isinstance(item, dict):
            runs.append(item)
    return runs


def evaluate_ci_gate(cwd: Path) -> tuple[bool, list[str]]:
    head = _current_head(cwd)
    pr = _current_pr(cwd)
    pr_head = pr.get("headRefOid")
    pr_url = pr.get("url", "unknown PR")
    messages = [f"PR: {pr_url}", f"HEAD: {head}"]

    if pr_head != head:
        messages.append(f"FAIL: PR head is {pr_head}; push current HEAD before relying on CI.")
        return False, messages

    runs = _workflow_runs(cwd, head)
    by_name = {str(run.get("name")): run for run in runs if run.get("headSha") in (None, head)}
    ok = True
    saw_accepted_workflow = False
    for workflow in ACCEPTED_WORKFLOWS:
        run = by_name.get(workflow)
        if run is None:
            messages.append(f"INFO: no workflow run for {workflow!r} at current HEAD.")
            continue
        saw_accepted_workflow = True
        status = run.get("status")
        conclusion = run.get("conclusion")
        if status == "completed" and conclusion == "success":
            messages.append(f"PASS: {workflow} completed successfully.")
        else:
            messages.append(f"FAIL: {workflow} status={status!r} conclusion={conclusion!r}.")
            ok = False
    if not saw_accepted_workflow:
        messages.append("FAIL: no accepted GitHub CI workflow run exists for current HEAD.")
        ok = False
    return ok, messages


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=VALID_KINDS)
    parser.add_argument("--repo", default=".", help="Repository checkout to evaluate.")
    args = parser.parse_args(argv)

    repo = Path(args.repo).resolve()
    try:
        ok, messages = evaluate_ci_gate(repo)
    except (OSError, RuntimeError) as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        print(
            "Open or update the PR, wait for exact-head GitHub CI, or run an explicitly approved managed local check.",
            file=sys.stderr,
        )
        return 1

    print(f"no-mistakes {args.kind}: exact-head GitHub CI gate")
    for message in messages:
        print(message)
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
