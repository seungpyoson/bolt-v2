#!/usr/bin/env python3
"""Verify RA-009 Polymarket cost realism is wired into the BTE venue config."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
RUN_MANIFEST_PATH = Path("crates/backtesting-vertical-slice/src/run_manifest.rs")
CARGO_PATH = Path("crates/backtesting-vertical-slice/Cargo.toml")
TEST_PATH = RUN_MANIFEST_PATH
JUSTFILE_PATH = Path("justfile")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")

CHECKED_RA009 = re.compile(r"^- \[[xX]\] RA-009\b", re.MULTILINE)


def strip_rust_comments_and_literals(text: str) -> str:
    out: list[str] = []
    i = 0
    state = "code"
    block_depth = 0
    while i < len(text):
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


def require_pattern(rel_path: Path, text: str, label: str, pattern: str, findings: list[str]) -> None:
    if not re.search(pattern, text, re.DOTALL):
        findings.append(f"{rel_path}: missing real {label}")


def unsupported_surface_block(text: str) -> str:
    match = re.search(
        r"pub\s+const\s+UNSUPPORTED_NT_VENUE_SURFACES\s*:\s*&\[\s*&str\s*\]\s*=\s*&\[(.*?)\];",
        text,
        re.DOTALL,
    )
    return match.group(1) if match else ""


def scan_root(root: Path) -> list[str]:
    findings: list[str] = []
    run_manifest_text = (
        (root / RUN_MANIFEST_PATH).read_text(encoding="utf-8")
        if (root / RUN_MANIFEST_PATH).exists()
        else ""
    )
    cargo_text = (root / CARGO_PATH).read_text(encoding="utf-8") if (root / CARGO_PATH).exists() else ""
    test_text = (root / TEST_PATH).read_text(encoding="utf-8") if (root / TEST_PATH).exists() else ""
    just_text = (
        (root / JUSTFILE_PATH).read_text(encoding="utf-8") if (root / JUSTFILE_PATH).exists() else ""
    )
    tasks_text = (
        (root / TASKS_PATH).read_text(encoding="utf-8") if (root / TASKS_PATH).exists() else ""
    )

    code = strip_rust_comments_and_literals(run_manifest_text)
    test_code = strip_rust_comments_and_literals(test_text)

    if not CHECKED_RA009.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-009 must be checked only when cost realism is implemented")

    if "nautilus-execution" not in cargo_text:
        findings.append(f"{CARGO_PATH}: missing direct nautilus-execution dependency")

    for label, pattern in (
        ("ManifestFillModelConfig", r"\bpub\s+struct\s+ManifestFillModelConfig\b"),
        ("ManifestLatencyModelConfig", r"\bpub\s+struct\s+ManifestLatencyModelConfig\b"),
        ("ManifestFeeModelConfig", r"\bpub\s+struct\s+ManifestFeeModelConfig\b"),
        ("fill model resolver", r"\bfn\s+resolve_fill_model\b.*\bProbabilisticFillModel\s*::\s*new\s*\(.*\bFillModelAny\s*::\s*Probabilistic\b"),
        ("probabilistic fill model seed guard", r"\brandom_seed\b\s*\.\s*ok_or\s*\(\s*ManifestError\s*::\s*MissingField\s*\("),
        ("latency model resolver", r"\bfn\s+resolve_latency_model\b.*\bLatencyModelAny\s*::\s*Static\b.*\bStaticLatencyModel\s*::\s*new\s*\("),
        ("fee model resolver", r"\bfn\s+resolve_fee_model\b.*\bFeeModelAny\s*::\s*MakerTaker\s*\(\s*MakerTakerFeeModel\s*\)"),
        ("BTE fill registration", r"\.maybe_fill_model\s*\(\s*resolve_fill_model\s*\("),
        ("BTE latency registration", r"\.maybe_latency_model\s*\(\s*resolve_latency_model\s*\("),
        ("BTE fee registration", r"\.maybe_fee_model\s*\(\s*resolve_fee_model\s*\("),
        ("venue model validation", r"\bresolve_fill_model\s*\(\s*manifest\s*\.\s*venue\s*\.\s*fill_model\s*\.\s*as_ref\s*\(\s*\)\s*\)\s*\?"),
        ("durable fill surface", r"\bresolved_surface\s*\(\s*\"\"\s*,\s*NtSurfaceClassification\s*::\s*PassThrough\s*,\s*\"\"\s*,\s*option_value\s*\(\s*venue\s*\.\s*fill_model\s*\(\s*\)\s*\)"),
    ):
        require_pattern(RUN_MANIFEST_PATH, code, label, pattern, findings)

    block = unsupported_surface_block(run_manifest_text)
    if not block:
        findings.append(f"{RUN_MANIFEST_PATH}: missing UNSUPPORTED_NT_VENUE_SURFACES declaration")
    for model_field in ('"fill_model"', '"latency_model"', '"fee_model"'):
        if model_field in block:
            findings.append(f"{RUN_MANIFEST_PATH}: {model_field} must not remain unsupported")

    for label, pattern in (
        ("positive cost-realism mapping test", r"\bfn\s+venue_config_registers_polymarket_cost_realism_models_with_nt\b"),
        ("unknown selector rejection test", r"\bfn\s+rejects_unknown_polymarket_cost_realism_model_selectors\b"),
        ("invalid parameter rejection test", r"\bfn\s+rejects_invalid_polymarket_cost_realism_parameters\b"),
    ):
        require_pattern(TEST_PATH, test_code, label, pattern, findings)

    for command in (
        "python3 scripts/test_verify_ra_cost_realism.py",
        "python3 scripts/verify_ra_cost_realism.py",
    ):
        if command not in just_text:
            findings.append(f"{JUSTFILE_PATH}: source-fence-static must run {command}")

    return findings


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args()
    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA cost realism violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA cost realism passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
