#!/usr/bin/env python3
"""Verify BTE-022 binary-option Bar catalog evidence stays source-fenced."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
REFERENCE_ROOT = REPO_ROOT / "specs/023-nt-research-analytics-platform/reference"

STATUS = REFERENCE_ROOT / "source-proof-bte-022-binary-option-bar-catalog-status.2026-06-16.json"
BTE_022_STATUS = (
    REFERENCE_ROOT / "source-proof-nt-catalog-mapping-status.backtesting-engine-022.2026-06-08.json"
)
MAPPING_EVALUATION = (
    REFERENCE_ROOT / "source-proof-nt-catalog-mapping-evaluation.backtesting-engine.2026-06-08.json"
)
READINESS_REPORT = (
    REFERENCE_ROOT
    / "source-catalog-mapping-readiness/polymarket-parquet-archive-index-canonical/source-catalog-mapping-readiness-report.json"
)
CATALOG_PROJECTION = REPO_ROOT / "crates/backtesting-vertical-slice/src/catalog_projection.rs"
RUN_MANIFEST = REPO_ROOT / "crates/backtesting-vertical-slice/src/run_manifest.rs"
JUSTFILE = REPO_ROOT / "justfile"

REQUIRED_SOURCE_FENCE_COMMANDS = (
    "python3 scripts/test_verify_bte_022_binary_option_bar_catalog.py",
    "python3 scripts/verify_bte_022_binary_option_bar_catalog.py",
)
SOURCE_FENCE_STATIC_COMMANDS = ("python3 scripts/run_fences.py",)

CATALOG_PROJECTION_REQUIRED_SOURCE_SNIPPETS = (
    "fn binary_option_bar_catalog_projection_round_trips_through_nt_catalog()",
    "project_canonical_bars_to_catalog(",
    "&binary_option_spec()",
    "read_back_bars(dir.path(),",
    "InstrumentAny::BinaryOption",
    "NT_DATA_TYPE_BAR",
    "SourceProofFidelityClass::TradeBarReplay",
)

RUN_MANIFEST_REQUIRED_SOURCE_SNIPPETS = (
    "data_type: NautilusDataType::Bar",
    "fidelity: SourceProofFidelityClass::TradeBarReplay",
    "fn trade_bar_replay_accepts_bar_data_config()",
)


def load_json(path: Path) -> Any:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def require_equal(path: Path | str, field: str, actual: Any, expected: Any, findings: list[str]) -> None:
    if actual != expected:
        findings.append(f"{path}: {field} expected {expected!r}, got {actual!r}")


def require_in(path: Path | str, field: str, value: Any, values: Any, findings: list[str]) -> None:
    if not isinstance(values, list) or value not in values:
        findings.append(f"{path}: {field} must include {value!r}, got {values!r}")


def require_not_in(path: Path | str, field: str, value: Any, values: Any, findings: list[str]) -> None:
    if isinstance(values, list) and value in values:
        findings.append(f"{path}: {field} must not include stale value {value!r}")


def require_text(path: Path, text: str, needle: str, findings: list[str]) -> None:
    if needle not in text:
        findings.append(f"{path}: missing required source-fence text {needle!r}")


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def raw_string_end(text: str, i: int) -> int | None:
    if text.startswith("br", i):
        i += 2
    elif text.startswith("r", i):
        i += 1
    else:
        return None
    hashes = 0
    while i < len(text) and text[i] == "#":
        hashes += 1
        i += 1
    if i >= len(text) or text[i] != '"':
        return None
    closing = '"' + ("#" * hashes)
    end = text.find(closing, i + 1)
    if end == -1:
        return len(text)
    return end + len(closing)


def strip_rust_comments_and_literals(text: str) -> str:
    out: list[str] = []
    i = 0
    state = "code"
    block_depth = 0
    while i < len(text):
        raw_end = raw_string_end(text, i)
        if state == "code" and raw_end is not None:
            out.append('""')
            out.extend("\n" for _ in range(text.count("\n", i, raw_end)))
            i = raw_end
            continue
        c = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""
        if state == "code":
            if c == "/" and nxt == "/":
                state = "line_comment"
                out.extend("  ")
                i += 2
                continue
            if c == "/" and nxt == "*":
                state = "block_comment"
                block_depth = 1
                out.extend("  ")
                i += 2
                continue
            if c == '"':
                state = "string"
                out.extend('""')
                i += 1
                continue
            out.append(c)
            i += 1
            continue
        if state == "line_comment":
            if c == "\n":
                state = "code"
                out.append(c)
            else:
                out.append(" ")
            i += 1
            continue
        if state == "block_comment":
            if c == "/" and nxt == "*":
                block_depth += 1
                out.extend("  ")
                i += 2
                continue
            if c == "*" and nxt == "/":
                block_depth -= 1
                out.extend("  ")
                i += 2
                if block_depth == 0:
                    state = "code"
                continue
            out.append("\n" if c == "\n" else " ")
            i += 1
            continue
        if state == "string":
            if c == "\\":
                i += 2
                continue
            if c == '"':
                state = "code"
            i += 1
            continue
    return "".join(out)


def verify_status(status: dict[str, Any], findings: list[str]) -> None:
    require_equal(
        STATUS,
        "schema_version",
        status.get("schema_version"),
        "source-proof-bte-022-binary-option-bar-catalog-status.v1",
        findings,
    )
    require_equal(STATUS, "task_id", status.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(STATUS, "source_binding", status.get("source_binding"), "kalshi-official-historical-api", findings)
    require_equal(STATUS, "fixture_type", status.get("fixture_type"), "binary-option", findings)
    require_equal(STATUS, "table_family", status.get("table_family"), "bars", findings)
    require_equal(STATUS, "fidelity_class", status.get("fidelity_class"), "TRADE_BAR_REPLAY", findings)
    require_equal(STATUS, "nt_data_class", status.get("nt_data_class"), "Bar", findings)
    require_equal(
        STATUS,
        "current_bte_manifest_status",
        status.get("current_bte_manifest_status"),
        "bar_admitted_under_trade_bar_replay",
        findings,
    )
    require_equal(
        STATUS,
        "parquet_catalog_status",
        status.get("parquet_catalog_status"),
        "binary_option_bar_projection_readback_source_fenced",
        findings,
    )
    require_equal(STATUS, "bte_022_can_close", status.get("bte_022_can_close"), False, findings)

    scope = status.get("scope", {})
    require_equal(STATUS, "scope.not_source_selection", scope.get("not_source_selection"), True, findings)
    require_equal(
        STATUS,
        "scope.not_backfill_authorization",
        scope.get("not_backfill_authorization"),
        True,
        findings,
    )
    require_equal(STATUS, "scope.not_bte_022_closure", scope.get("not_bte_022_closure"), True, findings)

    evidence = status.get("evidence_refs", {})
    for key, expected in {
        "catalog_projection_test": "repo://crates/backtesting-vertical-slice/src/catalog_projection.rs#binary_option_bar_catalog_projection_round_trips_through_nt_catalog",
        "catalog_projection_function": "repo://crates/backtesting-vertical-slice/src/catalog_projection.rs#project_canonical_bars_to_catalog",
        "catalog_readback_function": "repo://crates/backtesting-vertical-slice/src/catalog_projection.rs#read_back_bars",
        "manifest_admittance_table": "repo://crates/backtesting-vertical-slice/src/run_manifest.rs#ADMITTANCE_TABLE",
    }.items():
        require_equal(STATUS, f"evidence_refs.{key}", evidence.get(key), expected, findings)

    guard = status.get("guard_verification", {})
    require_equal(
        STATUS,
        "guard_verification.script",
        guard.get("script"),
        "repo://scripts/verify_bte_022_binary_option_bar_catalog.py",
        findings,
    )
    require_equal(
        STATUS,
        "guard_verification.self_test",
        guard.get("self_test"),
        "repo://scripts/test_verify_bte_022_binary_option_bar_catalog.py",
        findings,
    )


def verify_code(catalog_projection_text: str, run_manifest_text: str, findings: list[str]) -> None:
    catalog_projection_code = strip_rust_comments_and_literals(catalog_projection_text)
    run_manifest_code = strip_rust_comments_and_literals(run_manifest_text)
    for needle in CATALOG_PROJECTION_REQUIRED_SOURCE_SNIPPETS:
        require_text(CATALOG_PROJECTION, catalog_projection_code, needle, findings)
    for needle in RUN_MANIFEST_REQUIRED_SOURCE_SNIPPETS:
        require_text(RUN_MANIFEST, run_manifest_code, needle, findings)


def mapping_for_binding(evaluation: dict[str, Any], source_binding: str) -> dict[str, Any]:
    for mapping in evaluation.get("source_sample_mapping_status", []):
        if mapping.get("source_binding") == source_binding:
            return mapping
    return {}


def verify_evaluation(evaluation: dict[str, Any], findings: list[str]) -> None:
    exposure = evaluation.get("nt_surface_evidence", {}).get("current_bte_manifest_exposure", {})
    require_in(MAPPING_EVALUATION, "current_bte_manifest_exposure.accepted_data_classes", "Bar", exposure.get("accepted_data_classes"), findings)
    require_not_in(MAPPING_EVALUATION, "current_bte_manifest_exposure.rejected_for_now", "Bar", exposure.get("rejected_for_now"), findings)

    evaluation_text = json.dumps(evaluation, sort_keys=True)
    if "BTE currently admits only TradeTick" in evaluation_text:
        findings.append(f"{MAPPING_EVALUATION}: stale Kalshi decision still says BTE admits only TradeTick")

    kalshi = mapping_for_binding(evaluation, "kalshi-official-historical-api")
    require_equal(
        MAPPING_EVALUATION,
        "kalshi.current_bte_status",
        kalshi.get("current_bte_status"),
        "staged_binary_option_bar_catalog_mapping_source_fenced",
        findings,
    )
    require_equal(
        MAPPING_EVALUATION,
        "kalshi.parquet_catalog_status",
        kalshi.get("parquet_catalog_status"),
        "binary_option_bar_projection_readback_source_fenced",
        findings,
    )
    require_in(
        MAPPING_EVALUATION,
        "kalshi.nt_data_class_evidence_refs.Bar",
        "repo://specs/023-nt-research-analytics-platform/reference/source-proof-bte-022-binary-option-bar-catalog-status.2026-06-16.json",
        kalshi.get("nt_data_class_evidence_refs", {}).get("Bar"),
        findings,
    )


def verify_hash_bindings(bte_status: dict[str, Any], readiness: dict[str, Any], findings: list[str]) -> None:
    expected_hash = sha256_file(MAPPING_EVALUATION)
    status_hash = (
        bte_status.get("current_reconciliation", {})
        .get("source_catalog_mapping_readiness_binding_status", {})
        .get("catalog_mapping_evaluation_hash")
    )
    require_equal(BTE_022_STATUS, "catalog_mapping_evaluation_hash", status_hash, expected_hash, findings)
    require_equal(
        READINESS_REPORT,
        "catalog_mapping_evaluation_hash",
        readiness.get("catalog_mapping_evaluation_hash"),
        expected_hash,
        findings,
    )


def recipe_body(justfile_text: str, recipe_name: str) -> list[str]:
    lines = justfile_text.splitlines()
    for index, line in enumerate(lines):
        if line.startswith(f"{recipe_name}:"):
            body: list[str] = []
            for candidate in lines[index + 1 :]:
                if candidate and not candidate.startswith((" ", "\t")):
                    break
                stripped = candidate.strip()
                if stripped:
                    body.append(stripped)
            return body
    return []


def verify_justfile(justfile_text: str, findings: list[str]) -> None:
    recipe = recipe_body(justfile_text, "verify-bte-022-binary-option-bar-catalog")
    if not recipe:
        findings.append(f"{JUSTFILE}: missing verify-bte-022-binary-option-bar-catalog recipe")
    source_fence = recipe_body(justfile_text, "source-fence-static-inner")
    for command in REQUIRED_SOURCE_FENCE_COMMANDS:
        if command not in recipe:
            findings.append(f"{JUSTFILE}: verify-bte-022-binary-option-bar-catalog must run {command}")
    for command in SOURCE_FENCE_STATIC_COMMANDS:
        if command not in source_fence:
            findings.append(f"{JUSTFILE}: source-fence-static-inner must run {command}")


def verify() -> list[str]:
    findings: list[str] = []
    verify_status(load_json(STATUS), findings)
    verify_code(CATALOG_PROJECTION.read_text(encoding="utf-8"), RUN_MANIFEST.read_text(encoding="utf-8"), findings)
    verify_evaluation(load_json(MAPPING_EVALUATION), findings)
    verify_hash_bindings(load_json(BTE_022_STATUS), load_json(READINESS_REPORT), findings)
    verify_justfile(JUSTFILE.read_text(encoding="utf-8"), findings)
    return findings


def main() -> int:
    findings = verify()
    if findings:
        print("FAIL: BTE-022 binary-option Bar catalog status violations:", file=sys.stderr)
        for finding in findings:
            print(f" - {finding}", file=sys.stderr)
        return 1
    print("OK: BTE-022 binary-option Bar catalog status passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
