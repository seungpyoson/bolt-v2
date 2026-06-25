#!/usr/bin/env python3
"""Tests for the Bolt-v3 decision-evidence v13 -> v14 migrator."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("migrate_bolt_v3_decision_evidence_v13_to_v14.py")
SPEC = importlib.util.spec_from_file_location("migrate_bolt_v3_decision_evidence_v13_to_v14", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
MIGRATOR = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MIGRATOR
SPEC.loader.exec_module(MIGRATOR)


def write_jsonl(path: Path, lines: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(("\n".join(lines) + "\n").encode("utf-8"))


def run_migration(directory: Path, *, dry_run: bool = False) -> dict[str, object]:
    argv = [str(directory)]
    if dry_run:
        argv.append("--dry-run")
    return MIGRATOR.migrate_cli(argv)


def test_migrates_v13_records_with_key_scoped_byte_preserving_replacements(tmp_path: Path) -> None:
    path = tmp_path / "decision-evidence" / "records.jsonl"
    lines = [
        '{"schema_version":13,"recorded_at_utc_ns":1731234567890123456,"gate_id":"bolt_v3.position_sizer_rebuild","kind":"position_sizer_rebuild","payload":{"source":"nt_position_sizer_runtime_components","unchanged":[1,13,"position_sizer_rebuild"]}}',
        '{"schema_version":13,"recorded_at_utc_ns":1700000000000000001,"gate_id":"bolt_v3.submit_admission","kind":"submit_reservation_metadata","payload":{"reservation_id":"reservation-one","source":"nt_sizing_state","unchanged":13}}',
        '{"schema_version":13,"recorded_at_utc_ns":1700000000000000002,"gate_id":"bolt_v3.submit_admission","kind":"admission_decision","payload":{"decision":{"outcome":"rejected_position_sizing"},"note":"unchanged"}}',
        '{"schema_version":13,"recorded_at_utc_ns":1700000000000000003,"gate_id":"bolt_v3.submit_admission","kind":"loss_snapshot","payload":{"source":"nt_sizing_state","note":"untouched-tail"}}',
    ]
    write_jsonl(path, lines)

    manifest = run_migration(tmp_path / "decision-evidence")

    migrated = path.read_text(encoding="utf-8").splitlines()
    assert migrated == [
        '{"schema_version":14,"recorded_at_utc_ns":1731234567890123456,"gate_id":"bolt_v3.capital_admission_rebuild","kind":"capital_admission_rebuild","payload":{"source":"nt_capital_admission_runtime_components","unchanged":[1,13,"position_sizer_rebuild"]}}',
        '{"schema_version":14,"recorded_at_utc_ns":1700000000000000001,"gate_id":"bolt_v3.submit_admission","kind":"submit_reservation_metadata","payload":{"reservation_id":"reservation-one","source":"nt_capital_admission_state","unchanged":13}}',
        '{"schema_version":14,"recorded_at_utc_ns":1700000000000000002,"gate_id":"bolt_v3.submit_admission","kind":"admission_decision","payload":{"decision":{"outcome":"rejected_capital_admission"},"note":"unchanged"}}',
        '{"schema_version":14,"recorded_at_utc_ns":1700000000000000003,"gate_id":"bolt_v3.submit_admission","kind":"loss_snapshot","payload":{"source":"nt_capital_admission_state","note":"untouched-tail"}}',
    ]
    assert manifest["changed_files"] == [
        {
            "path": str(path),
            "before_sha256": MIGRATOR.sha256_bytes(("\n".join(lines) + "\n").encode("utf-8")),
            "after_sha256": MIGRATOR.sha256_bytes(path.read_bytes()),
        }
    ]


def test_non_corruption_guard_preserves_timestamps_and_payload_string_values(tmp_path: Path) -> None:
    path = tmp_path / "records.jsonl"
    original = (
        '{"schema_version":13,"recorded_at_utc_ns":1731234567890123456,'
        '"gate_id":"bolt_v3.submit_admission","kind":"submit_reservation_metadata",'
        '"payload":{"strategy_id":"position_sizer_rebuild","client_order_id":"nt_sizing_state",'
        '"source":"nt_sizing_state","sequence":13}}\n'
    )
    path.write_bytes(original.encode("utf-8"))

    run_migration(tmp_path)

    migrated = path.read_text(encoding="utf-8")
    assert '"recorded_at_utc_ns":1731234567890123456' in migrated
    assert '"strategy_id":"position_sizer_rebuild"' in migrated
    assert '"client_order_id":"nt_sizing_state"' in migrated
    assert '"sequence":13' in migrated
    assert '"schema_version":14' in migrated
    assert '"source":"nt_capital_admission_state"' in migrated


def test_idempotent_second_run_is_noop(tmp_path: Path) -> None:
    path = tmp_path / "records.jsonl"
    write_jsonl(
        path,
        [
            '{"schema_version":13,"gate_id":"bolt_v3.position_sizer_rebuild","kind":"position_sizer_rebuild","payload":{}}',
        ],
    )

    first = run_migration(tmp_path)
    after_first = path.read_bytes()
    second = run_migration(tmp_path)

    assert len(first["changed_files"]) == 1
    assert second == {"changed_files": []}
    assert path.read_bytes() == after_first


def test_existing_v14_record_is_left_untouched_while_v13_records_complete(tmp_path: Path) -> None:
    path = tmp_path / "mixed.jsonl"
    v14 = '{"schema_version":14,"gate_id":"bolt_v3.capital_admission_rebuild","kind":"capital_admission_rebuild","payload":{"source":"nt_capital_admission_state"}}'
    v13 = '{"schema_version":13,"gate_id":"bolt_v3.position_sizer_rebuild","kind":"position_sizer_rebuild","payload":{}}'
    write_jsonl(path, [v14, v13])

    run_migration(tmp_path)

    assert path.read_text(encoding="utf-8").splitlines() == [
        v14,
        '{"schema_version":14,"gate_id":"bolt_v3.capital_admission_rebuild","kind":"capital_admission_rebuild","payload":{}}',
    ]


def test_dry_run_reports_manifest_without_mutating(tmp_path: Path) -> None:
    path = tmp_path / "records.jsonl"
    original = b'{"schema_version":13,"gate_id":"bolt_v3.position_sizer_rebuild","kind":"position_sizer_rebuild","payload":{}}\n'
    path.write_bytes(original)

    manifest = run_migration(tmp_path, dry_run=True)

    assert len(manifest["changed_files"]) == 1
    assert path.read_bytes() == original


def test_refuses_schema_versions_outside_13_and_14_without_writing(tmp_path: Path) -> None:
    path = tmp_path / "records.jsonl"
    path.write_text(
        '{"schema_version":12,"gate_id":"bolt_v3.submit_admission","kind":"submit_reservation_metadata","payload":{}}\n'
        '{"schema_version":15,"gate_id":"bolt_v3.submit_admission","kind":"submit_reservation_metadata","payload":{}}\n',
        encoding="utf-8",
    )
    before = path.read_bytes()

    try:
        run_migration(tmp_path)
    except MIGRATOR.MigrationError as error:
        message = str(error)
    else:
        raise AssertionError("schema versions outside 13/14 must be refused")

    assert "unsupported schema_version=12" in message
    assert path.read_bytes() == before


def test_cli_prints_manifest_json(tmp_path: Path, capsys) -> None:
    path = tmp_path / "records.jsonl"
    write_jsonl(
        path,
        [
            '{"schema_version":13,"gate_id":"bolt_v3.position_sizer_rebuild","kind":"position_sizer_rebuild","payload":{}}',
        ],
    )

    rc = MIGRATOR.main([str(tmp_path)])

    captured = capsys.readouterr()
    assert rc == 0
    payload = json.loads(captured.out)
    assert payload["changed_files"][0]["path"] == str(path)
