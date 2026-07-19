#!/usr/bin/env python3
"""Execute the fixed final-review evidence sequence from protected governance."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import signal
import subprocess
import sys
import tomllib
from collections.abc import Callable, Iterable
from dataclasses import dataclass

from workspace_registry import load_registry, reconcile_registry


Record = dict[str, object]
Executor = Callable[[tuple[str, ...], pathlib.Path, pathlib.Path, float], int]


@dataclass(frozen=True)
class Obligation:
    obligation_id: str
    command: tuple[str, ...]
    cwd: pathlib.Path


FINAL_REVIEW_PHASES = {
    "static": (
        Obligation(
            "preflight",
            (
                "python3",
                "{governance}/scripts/local_verification_gate.py",
                "preflight",
                "--",
                "python3",
                "{governance}/scripts/repo_preflight.py",
                "--governance",
                "{governance}",
                "--subject",
                "{subject}",
            ),
            pathlib.Path("governance"),
        ),
        Obligation("host-health", ("python3", "scripts/test_host_health_sampler.py"), pathlib.Path("subject")),
        Obligation("host-health-viewer", ("node", "scripts/test_host_health_viewer.mjs"), pathlib.Path("subject")),
    ),
    "root-analysis": (
        Obligation("root-clippy", ("python3", "{owner}", "cargo", "--repo", "{subject}", "--", "clippy", "--locked", "--", "-D", "warnings"), pathlib.Path("governance")),
        Obligation("root-aarch64", ("python3", "{owner}", "cargo", "--repo", "{subject}", "--", "check", "--target", "aarch64-unknown-linux-gnu", "--locked"), pathlib.Path("governance")),
        Obligation("root-build", ("python3", "{owner}", "cargo", "--repo", "{subject}", "--", "zigbuild", "--release", "--target", "aarch64-unknown-linux-gnu", "--locked"), pathlib.Path("governance")),
    ),
    "root-tests": (
        Obligation("root-archive", ("python3", "{owner}", "cargo", "--repo", "{subject}", "--", "nextest", "archive", "--locked", "--archive-file", "{root_archive}"), pathlib.Path("subject")),
        Obligation("root-cache-release", ("python3", "{owner}", "cargo", "--repo", "{subject}", "--", "clean"), pathlib.Path("governance")),
        Obligation("root-tests", ("python3", "{owner}", "cargo", "--repo", "{subject}", "--", "nextest", "run", "--archive-file", "{root_archive}", "--extract-to", ".nextest-root", "--extract-overwrite", "--workspace-remap", "{subject}", "--no-fail-fast"), pathlib.Path("subject")),
        Obligation("root-special-proofs", ("python3", "{owner}", "cargo", "--repo", "{subject}", "--", "nextest", "run", "--archive-file", "{root_archive}", "--extract-to", ".nextest-proofs", "--extract-overwrite", "--workspace-remap", "{subject}", "--no-tests=fail", "-E", "binary(=binance_sbe_quote_timestamps)"), pathlib.Path("subject")),
    ),
    "bvs-analysis": (
        Obligation("bvs-clippy", ("python3", "{owner}", "cargo", "--repo", "{bvs}", "--", "clippy", "--locked", "--", "-D", "warnings"), pathlib.Path("governance")),
    ),
    "bvs-tests": (
        Obligation("bvs-archive", ("python3", "{governance}/scripts/bvs_archive.py", "--owner", "{owner}", "--repo", "{bvs}", "--archive", "{bvs_archive}"), pathlib.Path("governance")),
        Obligation("bvs-cache-release", ("python3", "{owner}", "cargo", "--repo", "{bvs}", "--", "clean"), pathlib.Path("governance")),
        Obligation("bvs-s3-smoke", ("python3", "{owner}", "cargo", "--repo", "{bvs}", "--", "nextest", "run", "--archive-file", "{bvs_archive}", "--extract-to", ".nextest-bvs-s3", "--extract-overwrite", "--workspace-remap", "{bvs}", "--no-tests=fail", "backtesting_vertical_slice_s3_catalog_smoke"), pathlib.Path("subject")),
        Obligation("bvs-tests", ("python3", "{owner}", "cargo", "--repo", "{bvs}", "--", "nextest", "run", "--archive-file", "{bvs_archive}", "--extract-to", ".nextest-bvs", "--extract-overwrite", "--workspace-remap", "{bvs}", "--no-fail-fast", "--", "--skip", "backtesting_vertical_slice_s3_catalog_smoke"), pathlib.Path("subject")),
    ),
}

FINAL_REVIEW_OBLIGATIONS = tuple(
    obligation
    for phase_obligations in FINAL_REVIEW_PHASES.values()
    for obligation in phase_obligations
)


def workflow_configuration(config_path: pathlib.Path) -> tuple[object, ...]:
    with config_path.open("rb") as handle:
        config = tomllib.load(handle)
    final_review = config["final_review"]
    return (
        config["claude"]["workflow"]["job_timeout_minutes"],
        config["kimi"]["workflow"]["job_timeout_minutes"],
        config["glm"]["workflow"]["job_timeout_minutes"],
        final_review["obligation_timeout_seconds"],
        final_review["evidence_timeout_minutes"],
        final_review["python_version"],
        json.dumps({"phase": final_review["phases"]}, separators=(",", ":")),
    )


def registered_workspace_roots(governance: pathlib.Path, subject: pathlib.Path) -> dict[str, pathlib.Path]:
    registry = load_registry(governance)
    reconcile_registry(subject, registry)
    return {
        workspace.workspace_id: (subject / workspace.path).resolve()
        for workspace in registry.workspaces
    }


def render_obligations(phase: str, values: dict[str, str]) -> tuple[Obligation, ...]:
    return tuple(
        Obligation(
            obligation.obligation_id,
            tuple(part.format_map(values) for part in obligation.command),
            obligation.cwd,
        )
        for obligation in FINAL_REVIEW_PHASES[phase]
    )


def execute_command(command: tuple[str, ...], cwd: pathlib.Path, log: pathlib.Path, timeout_seconds: float) -> int:
    log.parent.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    environment.pop("GITHUB_TOKEN", None)
    environment.pop("GH_TOKEN", None)
    with log.open("w", encoding="utf-8") as stream:
        process = subprocess.Popen(
            command,
            cwd=cwd,
            env=environment,
            stdout=stream,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
        )
        timed_out = False
        try:
            return_code = process.wait(timeout=timeout_seconds)
        except subprocess.TimeoutExpired:
            timed_out = True
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            if process.poll() is None:
                process.wait()

        if timed_out:
            stream.write(f"command timed out after {timeout_seconds:g} seconds\n")
            raise TimeoutError(f"command timed out after {timeout_seconds:g} seconds") from None
        return return_code


def run_obligations(
    obligations: Iterable[Obligation],
    *,
    governance: pathlib.Path,
    subject: pathlib.Path,
    head_sha: str,
    run_id: str,
    run_attempt: str,
    output: pathlib.Path,
    timeout_seconds: float,
    execute: Executor = execute_command,
) -> list[Record]:
    output.mkdir(parents=True, exist_ok=True)
    roots = {"governance": governance, "subject": subject}
    obligation_list = tuple(obligations)
    expected = {
        "obligation_ids": [obligation.obligation_id for obligation in obligation_list],
        "head_sha": head_sha,
        "run_id": run_id,
        "run_attempt": run_attempt,
    }
    (output / "expected.json").write_text(json.dumps(expected), encoding="utf-8")
    records: list[Record] = []
    for obligation in obligation_list:
        cwd = roots[obligation.cwd.as_posix()]
        log = output / "logs" / f"{obligation.obligation_id}.log"
        log.parent.mkdir(parents=True, exist_ok=True)
        try:
            returncode = execute(obligation.command, cwd, log, timeout_seconds)
            conclusion = "success" if returncode == 0 else "failure"
        except Exception as exc:
            conclusion = "infrastructure_failure"
            with log.open("a", encoding="utf-8") as stream:
                stream.write(f"{type(exc).__name__}: {exc}\n")
        records.append(
            {
                "obligation_id": obligation.obligation_id,
                "head_sha": head_sha,
                "run_id": run_id,
                "run_attempt": run_attempt,
                "conclusion": conclusion,
                "artifact_path": log.relative_to(output).as_posix(),
            }
        )
    (output / "records.json").write_text(json.dumps(records), encoding="utf-8")
    return records


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--governance", required=True, type=pathlib.Path)
    parser.add_argument("--subject", required=True, type=pathlib.Path)
    parser.add_argument("--head-sha", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--run-attempt", required=True)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument("--phase", required=True, choices=tuple(FINAL_REVIEW_PHASES))
    parser.add_argument("--obligation-timeout-seconds", required=True, type=float)
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)
    governance = args.governance.resolve()
    subject = args.subject.resolve()
    workspace_roots = registered_workspace_roots(governance, subject)
    root = workspace_roots["bolt_v2"]
    bvs_root = workspace_roots["backtesting_vertical_slice"]
    owner = str(governance / "scripts/rust_verification.py")
    root_archive = str(root / ".nextest-archive/root.tar.zst")
    bvs_archive = str(bvs_root / ".nextest-archive/bvs.tar.zst")
    obligations = render_obligations(
        args.phase,
        {
            "governance": str(governance),
            "subject": str(root),
            "owner": owner,
            "root_archive": root_archive,
            "bvs_archive": bvs_archive,
            "bvs": str(bvs_root),
        }
    )
    run_obligations(
        obligations,
        governance=governance,
        subject=subject,
        head_sha=args.head_sha,
        run_id=args.run_id,
        run_attempt=args.run_attempt,
        output=args.output.resolve(),
        timeout_seconds=args.obligation_timeout_seconds,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
