#!/usr/bin/env python3
"""Verify Bolt-v3 status-map claims that can drift against source."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
STATUS_MAP = REPO_ROOT / "docs/bolt-v3/2026-04-28-source-grounded-status-map.md"
MAIN_RS = REPO_ROOT / "src/main.rs"
PROVIDER_LEAK_VERIFIER = REPO_ROOT / "scripts/verify_bolt_v3_provider_leaks.py"
PURE_RUST_VERIFIER = "scripts/verify_bolt_v3_pure_rust_runtime.py"
PURE_RUST_AREA_TERMS = ("python", "runtime")
SCRIPT_REF_RE = re.compile(r"(?<![A-Za-z0-9_./-])(scripts/[A-Za-z0-9_./-]+\.py)(?![A-Za-z0-9_./-])")
MISSING_EVIDENCE_VALUES = {"", "missing", "n/a", "none", "tbd", "todo"}
MISSING_EVIDENCE_PHRASES = (
    "missing evidence",
    "no source",
    "no test",
    "no verifier",
    "not found",
    "not implemented",
)

STALE_STATUS_MAP_PHRASES = (
    "| 7 | Bolt-v3 binary / CLI entrypoint | Missing |",
    "No production caller for `build_bolt_v3_live_node` found outside tests",
    "| 49 | Provider-leak verifier for adapter/secrets/registration | Missing |",
    "No verifier selected or wired",
    "The next implementation work should be the provider-leak verifier",
    "| 2 | TOML-owned runtime configuration | Partial |",
    "Full runtime hardcode audit is still missing, including non-v3 runtime paths.",
    "| 5 | Runtime values come from TOML | Partial |",
    "status remains partial because non-v3 runtime paths",
    "| 8 | SSM-only secret source | Partial |",
    "`ResolvedBoltV3VenueSecrets` is still a closed provider enum",
    "provider-owned secret binding remains open",
    "`tests/live_node_run.rs`",
    "Pre-v3 `src/platform/audit.rs` and `src/platform/reference.rs` still hold legacy venue/provider logic",
    "| 14 | Provider-specific adapter mapping behind provider modules | Partial |",
    "Extending to a new provider requires adding a variant to the closed `BoltV3VenueAdapterConfig` enum.",
    "| 15 | Provider-specific secret handling behind provider modules | Partial |",
    "resolved-secret enum variants",
    "| 16 | Provider-specific client factory registration behind provider modules | Partial |",
    "Extending to a new provider requires adding a variant to the closed `BoltV3RegisteredVenue` enum.",
    "Residual closed dispatch remains in adapter/secrets/registration paths",
    "Archetype construction is still not done",
    "| 12 | Archetype validation dispatch | Partial |",
    "Strategy runtime construction is not implemented",
    "| 30 | Concrete NT strategy construction | Missing |",
    "No Bolt-v3 archetype builder that returns/registers concrete NT `Strategy` found",
    "| 31 | Strategy data subscriptions through NT | Missing |",
    "No Bolt-v3 strategy runtime subscription path accepted",
    "Still located in adapter mapper; provider ownership boundary is unresolved.",
    "| 26 | Selected live market target stack | Missing |",
    "No accepted Bolt-v3 target-stack implementation/test found",
    "| 28 | Reference data through NT subscriptions | Missing for Bolt-v3 live proof |",
    "Decide how reference providers bind into Bolt-v3 after instrument filter/readiness.",
    "| 33 | Risk and sizing policy | Missing / validation only |",
    "No NT-wired runtime risk engine",
    "| 39 | Order construction using NT-native IDs/types | Missing |",
    "No Bolt-v3 order construction path accepted",
    "| 41 | Execution gate / kill switch | Missing |",
    "No Bolt-v3 execution gate accepted",
)

REQUIRED_STATUS_MAP_PHRASES = (
    "| 7 | Bolt-v3 binary / CLI entrypoint | Implemented for current binary path |",
    "src/main.rs` `Command::Run` loads `load_bolt_v3_config`",
    "`build_bolt_v3_live_node(&loaded)?`",
    "`run_bolt_v3_live_node(&mut node, &loaded).await?`",
    "| 49 | Provider-leak verifier for adapter/secrets/registration | Implemented as current source-scan gate |",
    "`scripts/verify_bolt_v3_provider_leaks.py`",
    "`scripts/test_verify_bolt_v3_provider_leaks.py`",
    "| 2 | TOML-owned runtime configuration | Implemented for current source coverage |",
    "| 3 | No Python runtime layer | Implemented as current source-scan gate |",
    "| 5 | Runtime values come from TOML | Implemented for current source coverage |",
    "all `src/**/*.rs` production files",
    "zero unclassified literals",
    "`tests/bolt_v3_production_entrypoint.rs`",
    "| 8 | SSM-only secret source | Implemented for current providers |",
    "`ProviderBinding::resolve_secrets`",
    "provider-owned resolved-secret and redaction interfaces",
    "`scripts/verify_bolt_v3_pure_rust_runtime.py`",
    "| 14 | Provider-specific adapter mapping behind provider modules | Implemented for current providers |",
    "`ProviderBinding::map_adapters`",
    "`BoltV3VenueAdapterConfig` is a provider-neutral struct",
    "| 15 | Provider-specific secret handling behind provider modules | Implemented for current providers |",
    "`ProviderBinding::resolve_secrets`",
    "`ResolvedBoltV3VenueSecrets` is a provider-neutral trait object",
    "| 16 | Provider-specific client factory registration behind provider modules | Implemented for current providers |",
    "`BoltV3RegisteredVenue` records data/execution booleans",
    "| 10 | Core archetype identity is configured key, not closed enum | Implemented for current strategy archetype |",
    "`StrategyRuntimeBinding`",
    "| 12 | Archetype validation and runtime dispatch | Implemented for current strategy archetype |",
    "| 30 | Concrete NT strategy construction | Implemented for current strategy archetype |",
    "`BinaryOracleEdgeTakerBuilder`",
    "| 31 | Strategy data subscriptions through NT | Implemented for current strategy path; live proof still missing |",
    "| 24 | Provider discovery/filter binding | Implemented for current provider/family path |",
    "`src/bolt_v3_providers/polymarket.rs` owns Polymarket market-slug filter construction",
    "| 26 | Selected live market target stack | Implemented for current strategy path; live proof still missing |",
    "`select_binary_option_market_from_target`",
    "configured family key",
    "| 28 | Reference data through NT subscriptions | Implemented for current strategy path; live proof still missing |",
    "| 33 | Risk and sizing policy | Implemented for current strategy/admission path; live proof still missing |",
    "`src/bolt_v3_submit_admission.rs` enforces the TOML-derived live canary max-notional cap before NT submit",
    "| 39 | Order construction using NT-native IDs/types | Implemented for current strategy path; live proof still missing |",
    "builds configured entry and exit NT orders from strategy `entry_order`/`exit_order` config",
    "| 41 | Execution gate / kill switch | Implemented for live canary submit gate; broader kill switch missing |",
    "`src/bolt_v3_live_canary_gate.rs` validates approval, no-submit readiness, order-count cap, and notional caps before runner entry",
)


@dataclass(frozen=True)
class StatusRow:
    number: str
    area: str
    status: str
    source_evidence: str
    test_evidence: str
    gap: str


def split_markdown_table_row(line: str) -> list[str]:
    stripped = line.strip()
    if stripped.startswith("|"):
        stripped = stripped[1:]
    if stripped.endswith("|"):
        stripped = stripped[:-1]

    cells: list[str] = []
    current: list[str] = []
    in_code = False
    escaped = False

    for char in stripped:
        if escaped:
            current.append(char if char == "|" else f"\\{char}")
            escaped = False
            continue
        if char == "\\":
            escaped = True
            continue
        if char == "`":
            in_code = not in_code
            current.append(char)
            continue
        if char == "|" and not in_code:
            cells.append("".join(current).strip())
            current = []
            continue
        current.append(char)

    if escaped:
        current.append("\\")
    cells.append("".join(current).strip())
    return cells


def parse_rows(text: str) -> list[StatusRow]:
    rows: list[StatusRow] = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|") or stripped.startswith("|---"):
            continue

        cells = split_markdown_table_row(stripped)
        if len(cells) != 6 or not cells[0].isdigit():
            continue

        rows.append(
            StatusRow(
                number=cells[0],
                area=cells[1],
                status=cells[2],
                source_evidence=cells[3],
                test_evidence=cells[4],
                gap=cells[5],
            )
        )
    return rows


def missing_evidence(value: str) -> bool:
    normalized = value.strip().lower()
    return normalized in MISSING_EVIDENCE_VALUES or any(phrase in normalized for phrase in MISSING_EVIDENCE_PHRASES)


def validate_pure_rust_row(row: StatusRow) -> list[str]:
    findings: list[str] = []
    if not all(term in row.area.lower() for term in PURE_RUST_AREA_TERMS):
        findings.append(f"row 3 area changed unexpectedly: {row.area!r}")
    if "missing verifier" in row.status.lower():
        findings.append("row 3 still says the pure Rust runtime verifier is missing")
    if PURE_RUST_VERIFIER not in row.test_evidence:
        findings.append(f"row 3 test/verifier evidence must cite `{PURE_RUST_VERIFIER}`")
    if "No dedicated verifier found" in row.test_evidence:
        findings.append("row 3 test evidence still says no dedicated verifier was found")
    return findings


def main() -> int:
    findings: list[str] = []
    status_map = STATUS_MAP.read_text(encoding="utf-8")
    main_rs = MAIN_RS.read_text(encoding="utf-8")
    rows = parse_rows(status_map)

    if not rows:
        findings.append(f"{STATUS_MAP.relative_to(REPO_ROOT)}: no status rows parsed")

    by_number = {row.number: row for row in rows}
    pure_rust = by_number.get("3")
    if pure_rust is None:
        findings.append("status row 3 for no Python runtime layer is missing")
    else:
        findings.extend(validate_pure_rust_row(pure_rust))

    for row in rows:
        status = row.status.lower()
        if status.startswith("implemented") or status.startswith("partial"):
            if missing_evidence(row.source_evidence):
                findings.append(f"row {row.number} {row.area!r}: status {row.status!r} lacks source evidence")
            if missing_evidence(row.test_evidence):
                findings.append(f"row {row.number} {row.area!r}: status {row.status!r} lacks test/verifier evidence")

    for rel in sorted(set(SCRIPT_REF_RE.findall(status_map))):
        if not (REPO_ROOT / rel).exists():
            findings.append(f"{STATUS_MAP.relative_to(REPO_ROOT)} references missing verifier `{rel}`")

    for phrase in STALE_STATUS_MAP_PHRASES:
        if phrase in status_map:
            findings.append(f"status map still contains stale phrase: {phrase}")

    for phrase in REQUIRED_STATUS_MAP_PHRASES:
        if phrase not in status_map:
            findings.append(f"status map missing current evidence phrase: {phrase}")

    for phrase in (
        "load_bolt_v3_config(&config)?",
        "build_bolt_v3_live_node(&loaded)?",
        "run_bolt_v3_live_node(&mut node, &loaded).await?",
    ):
        if phrase not in main_rs:
            findings.append(f"src/main.rs missing expected bolt-v3 entrypoint phrase: {phrase}")

    if not PROVIDER_LEAK_VERIFIER.exists():
        findings.append("provider-leak verifier script is missing")
    elif "OK: Bolt-v3 provider-leak verifier passed." not in PROVIDER_LEAK_VERIFIER.read_text(
        encoding="utf-8"
    ):
        findings.append("provider-leak verifier script missing expected success marker")

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 status map matches current entrypoint and verifier evidence.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
