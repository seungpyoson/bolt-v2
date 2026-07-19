#!/usr/bin/env python3
"""Build a factual evidence manifest for the fixed final-review workflow."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
from collections.abc import Mapping, Sequence
from dataclasses import asdict, dataclass

from final_review_runner import FINAL_REVIEW_PHASES


class EvidenceError(RuntimeError):
    """Raised when evidence cannot be bound to one fixed workflow head."""


@dataclass(frozen=True)
class JobEvidence:
    obligation_id: str
    head_sha: str
    run_id: str
    run_attempt: str
    conclusion: str
    artifact_path: str


ALLOWED_CONCLUSIONS = frozenset(("success", "failure", "infrastructure_failure"))


def _required_text(record: Mapping[str, object], key: str) -> str:
    value = record.get(key)
    if not isinstance(value, str) or not value:
        raise EvidenceError(f"{key} must be a non-empty string")
    return value


def _parse_record(record: Mapping[str, object]) -> JobEvidence:
    return JobEvidence(
        obligation_id=_required_text(record, "obligation_id"),
        head_sha=_required_text(record, "head_sha"),
        run_id=_required_text(record, "run_id"),
        run_attempt=_required_text(record, "run_attempt"),
        conclusion=_required_text(record, "conclusion"),
        artifact_path=_required_text(record, "artifact_path"),
    )


def merge_phase_evidence(parts_root: pathlib.Path) -> tuple[dict[str, object], list[dict[str, object]]]:
    expected_phases = tuple(FINAL_REVIEW_PHASES)
    artifact_names = {phase: f"final-review-phase-{phase}" for phase in expected_phases}
    actual_phases = tuple(sorted(path.name for path in parts_root.iterdir() if path.is_dir()))
    if tuple(sorted(artifact_names.values())) != actual_phases:
        raise EvidenceError(f"evidence phases must be exactly {expected_phases!r}")

    identity: tuple[str, str, str] | None = None
    obligation_ids: list[str] = []
    records: list[dict[str, object]] = []
    for phase, obligations in FINAL_REVIEW_PHASES.items():
        artifact_name = artifact_names[phase]
        phase_root = parts_root / artifact_name
        try:
            envelope = json.loads((phase_root / "expected.json").read_text(encoding="utf-8"))
            raw_records = json.loads((phase_root / "records.json").read_text(encoding="utf-8"))
        except (FileNotFoundError, json.JSONDecodeError) as exc:
            raise EvidenceError(f"invalid evidence envelope for phase {phase}") from exc
        if not isinstance(envelope, dict) or not isinstance(raw_records, list):
            raise EvidenceError(f"invalid evidence shape for phase {phase}")

        phase_ids = tuple(obligation.obligation_id for obligation in obligations)
        if tuple(envelope.get("obligation_ids", ())) != phase_ids:
            raise EvidenceError(f"wrong obligation inventory for phase {phase}")
        phase_identity = (
            _required_text(envelope, "head_sha"),
            _required_text(envelope, "run_id"),
            _required_text(envelope, "run_attempt"),
        )
        if identity is None:
            identity = phase_identity
        elif phase_identity != identity:
            raise EvidenceError(f"mixed run identity for phase {phase}")

        obligation_ids.extend(phase_ids)
        record_ids = tuple(
            _required_text(raw, "obligation_id")
            for raw in raw_records
            if isinstance(raw, dict)
        )
        if len(record_ids) != len(raw_records) or record_ids != phase_ids:
            raise EvidenceError(f"wrong evidence records for phase {phase}")
        for raw in raw_records:
            if not isinstance(raw, dict):
                raise EvidenceError(f"invalid evidence record for phase {phase}")
            record = dict(raw)
            artifact_path = _required_text(record, "artifact_path")
            record["artifact_path"] = f"{artifact_name}/{artifact_path}"
            records.append(record)

    if identity is None:
        raise EvidenceError("final-review phase inventory is empty")
    return (
        {
            "obligation_ids": obligation_ids,
            "head_sha": identity[0],
            "run_id": identity[1],
            "run_attempt": identity[2],
        },
        records,
    )


def build_manifest(
    expected_ids: tuple[str, ...],
    records: Sequence[Mapping[str, object]],
    *,
    expected_head: str,
    expected_run_id: str,
    expected_run_attempt: str,
    evidence_root: pathlib.Path,
) -> dict[str, object]:
    if not expected_ids or len(set(expected_ids)) != len(expected_ids):
        raise EvidenceError("expected obligation IDs must be non-empty and unique")

    parsed: dict[str, JobEvidence] = {}
    for raw in records:
        record = _parse_record(raw)
        if record.obligation_id not in expected_ids:
            raise EvidenceError(f"unexpected obligation ID: {record.obligation_id}")
        if record.obligation_id in parsed:
            raise EvidenceError(f"duplicate obligation evidence: {record.obligation_id}")
        if record.head_sha != expected_head:
            raise EvidenceError(f"wrong head SHA for {record.obligation_id}")
        if record.run_id != expected_run_id or record.run_attempt != expected_run_attempt:
            raise EvidenceError(f"wrong run identity for {record.obligation_id}")
        if record.conclusion not in ALLOWED_CONCLUSIONS:
            raise EvidenceError(f"unsupported conclusion for {record.obligation_id}: {record.conclusion}")
        artifact = evidence_root / record.artifact_path
        try:
            resolved = artifact.resolve(strict=True)
            resolved.relative_to(evidence_root.resolve(strict=True))
        except (FileNotFoundError, RuntimeError, ValueError) as exc:
            raise EvidenceError(f"invalid artifact for {record.obligation_id}: {record.artifact_path}") from exc
        if artifact.is_symlink() or not resolved.is_file():
            raise EvidenceError(f"artifact must be a contained regular file: {record.artifact_path}")
        parsed[record.obligation_id] = record

    jobs: list[dict[str, object]] = []
    for obligation_id in expected_ids:
        record = parsed.get(obligation_id)
        if record is None:
            jobs.append(
                {
                    "obligation_id": obligation_id,
                    "head_sha": expected_head,
                    "run_id": expected_run_id,
                    "run_attempt": expected_run_attempt,
                    "conclusion": "missing",
                    "artifact_path": "missing",
                }
            )
        else:
            job = asdict(record)
            job["artifact_sha256"] = hashlib.sha256(
                (evidence_root / record.artifact_path).read_bytes()
            ).hexdigest()
            jobs.append(job)
    return {
        "schema_version": 1,
        "head_sha": expected_head,
        "run_id": expected_run_id,
        "run_attempt": expected_run_attempt,
        "jobs": jobs,
    }


def render_markdown(manifest: Mapping[str, object]) -> str:
    head = manifest.get("head_sha", "missing")
    lines = ["# Final review evidence", "", f"Head: `{head}`", ""]
    jobs = manifest.get("jobs")
    if not isinstance(jobs, list):
        raise EvidenceError("manifest jobs must be a list")
    for raw in jobs:
        if not isinstance(raw, dict):
            raise EvidenceError("manifest job must be an object")
        obligation_id = _required_text(raw, "obligation_id")
        conclusion = _required_text(raw, "conclusion")
        lines.append(f"- `{obligation_id}`: `{conclusion}`")
        artifact_path = _required_text(raw, "artifact_path")
        lines.append(f"  - log: `{artifact_path}`")
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--parts-root", required=True, type=pathlib.Path)
    parser.add_argument("--json-output", required=True, type=pathlib.Path)
    parser.add_argument("--markdown-output", required=True, type=pathlib.Path)
    args = parser.parse_args()

    envelope, records = merge_phase_evidence(args.parts_root)
    expected = tuple(envelope.get("obligation_ids", ()))
    manifest = build_manifest(
        expected,
        records,
        expected_head=_required_text(envelope, "head_sha"),
        expected_run_id=_required_text(envelope, "run_id"),
        expected_run_attempt=_required_text(envelope, "run_attempt"),
        evidence_root=args.parts_root,
    )
    (args.parts_root / "expected.json").write_text(
        json.dumps(envelope, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    (args.parts_root / "records.json").write_text(
        json.dumps(records, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    args.json_output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    args.markdown_output.write_text(render_markdown(manifest), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
