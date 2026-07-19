#!/usr/bin/env python3
"""Self-tests for raw fixed-job final-review evidence."""

from __future__ import annotations

import pathlib
import tempfile

from final_review_evidence import EvidenceError, build_manifest, render_markdown


HEAD = "a" * 40
EXPECTED = ("root-build", "root-tests", "bvs-tests")


def record(obligation_id: str, conclusion: str = "success", **extra: object) -> dict[str, object]:
    value: dict[str, object] = {
        "obligation_id": obligation_id,
        "head_sha": HEAD,
        "run_id": "42",
        "run_attempt": "1",
        "conclusion": conclusion,
        "artifact_path": f"logs/{obligation_id}.log",
    }
    value.update(extra)
    return value


def assert_missing_and_failed_jobs_remain_raw_evidence() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        for obligation_id in ("root-build", "root-tests"):
            path = root / "logs" / f"{obligation_id}.log"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("observed\n", encoding="utf-8")
        manifest = build_manifest(
            EXPECTED,
            (record("root-build"), record("root-tests", "failure")),
            expected_head=HEAD,
            expected_run_id="42",
            expected_run_attempt="1",
            evidence_root=root,
        )
    jobs = {job["obligation_id"]: job for job in manifest["jobs"]}
    if jobs["root-tests"]["artifact_path"] != "logs/root-tests.log":
        raise AssertionError(jobs)
    if jobs["bvs-tests"]["conclusion"] != "missing":
        raise AssertionError(jobs)
    rendered = render_markdown(manifest)
    for forbidden in ("CERTIFIED", "UNCERTIFIED", "CLEAR", "BLOCKED", "INCONCLUSIVE"):
        if forbidden in rendered:
            raise AssertionError(rendered)


def assert_mixed_heads_and_duplicates_are_rejected() -> None:
    cases = (
        (record("root-build"), record("root-tests", head_sha="b" * 40)),
        (record("root-build"), record("root-build")),
    )
    for records in cases:
        try:
            build_manifest(
                EXPECTED,
                records,
                expected_head=HEAD,
                expected_run_id="42",
                expected_run_attempt="1",
                evidence_root=pathlib.Path("."),
            )
        except EvidenceError:
            continue
        raise AssertionError(f"invalid records accepted: {records!r}")


def assert_wrong_identity_conclusion_and_path_are_rejected() -> None:
    cases = (
        record("root-build", head_sha="b" * 40),
        record("root-build", run_id="99"),
        record("root-build", run_attempt="2"),
        record("root-build", conclusion="neutral"),
        record("root-build", artifact_path="../outside.log"),
        record("root-build", artifact_path="logs/missing.log"),
    )
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        valid_log = root / "logs/root-build.log"
        valid_log.parent.mkdir(parents=True)
        valid_log.write_text("observed\n", encoding="utf-8")
        for invalid in cases:
            try:
                build_manifest(
                    ("root-build",),
                    (invalid,),
                    expected_head=HEAD,
                    expected_run_id="42",
                    expected_run_attempt="1",
                    evidence_root=root,
                )
            except EvidenceError:
                continue
            raise AssertionError(f"invalid evidence accepted: {invalid!r}")


def main() -> int:
    assert_missing_and_failed_jobs_remain_raw_evidence()
    assert_mixed_heads_and_duplicates_are_rejected()
    assert_wrong_identity_conclusion_and_path_are_rejected()
    print("OK: final-review evidence tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
