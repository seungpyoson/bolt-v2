#!/usr/bin/env python3
"""Self-tests for raw fixed-job final-review evidence."""

from __future__ import annotations

import json
import pathlib
import tempfile

from final_review_evidence import EvidenceError, build_manifest, merge_phase_evidence, render_markdown
from test_final_review_runner import EXPECTED_PHASES


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


def assert_phase_evidence_is_combined_without_inherited_or_missing_results() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        expected_ids: list[str] = []
        for phase, obligation_ids in EXPECTED_PHASES.items():
            phase_root = root / f"final-review-phase-{phase}"
            logs = phase_root / "logs"
            logs.mkdir(parents=True)
            (phase_root / "expected.json").write_text(
                json.dumps(
                    {
                        "obligation_ids": list(obligation_ids),
                        "head_sha": HEAD,
                        "run_id": "42",
                        "run_attempt": "1",
                    }
                ),
                encoding="utf-8",
            )
            records = []
            for obligation_id in obligation_ids:
                expected_ids.append(obligation_id)
                (logs / f"{obligation_id}.log").write_text("observed\n", encoding="utf-8")
                records.append(record(obligation_id))
            (phase_root / "records.json").write_text(
                json.dumps(records),
                encoding="utf-8",
            )

        envelope, records = merge_phase_evidence(root)
        if envelope["obligation_ids"] != expected_ids:
            raise AssertionError(envelope)
        if any(not str(item["artifact_path"]).startswith(tuple(f"final-review-phase-{phase}/" for phase in EXPECTED_PHASES)) for item in records):
            raise AssertionError(records)

        static_records_path = root / "final-review-phase-static" / "records.json"
        static_records = json.loads(static_records_path.read_text(encoding="utf-8"))
        static_records[0]["obligation_id"] = "root-clippy"
        static_records_path.write_text(json.dumps(static_records), encoding="utf-8")
        try:
            merge_phase_evidence(root)
        except EvidenceError:
            pass
        else:
            raise AssertionError("cross-phase obligation evidence was accepted")
        static_records[0]["obligation_id"] = "preflight"
        static_records_path.write_text(json.dumps(static_records), encoding="utf-8")

        missing_phase = root / "final-review-phase-bvs-tests"
        renamed_phase = root / "final-review-phase-bvs-tests-missing"
        missing_phase.rename(renamed_phase)
        try:
            merge_phase_evidence(root)
        except EvidenceError:
            pass
        else:
            raise AssertionError("missing phase evidence was accepted")


def main() -> int:
    assert_missing_and_failed_jobs_remain_raw_evidence()
    assert_mixed_heads_and_duplicates_are_rejected()
    assert_wrong_identity_conclusion_and_path_are_rejected()
    assert_phase_evidence_is_combined_without_inherited_or_missing_results()
    print("OK: final-review evidence tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
