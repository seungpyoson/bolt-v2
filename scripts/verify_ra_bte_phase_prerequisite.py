#!/usr/bin/env python3
"""Verify RA-016 wires the binary-oracle BTE prerequisite."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
PLAN_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/plan.md")
SPEC_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/spec.md")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")
BTE_CARGO_TOML = Path("crates/backtesting-vertical-slice/Cargo.toml")
BTE_RUN_MANIFEST = Path("crates/backtesting-vertical-slice/src/run_manifest.rs")
BTE_RUNNER = Path("crates/backtesting-vertical-slice/src/runner.rs")

PREREQUISITE_MARKER = "ra-bte-prerequisite-ids"
PREREQUISITE_REQUIRED_IDS = (
    "nt_example_strategy_current",
    "binary_oracle_edge_taker_required",
    "venue_normalization_required",
)
CHECKED_RA016 = re.compile(r"^- \[[xX]\] RA-016\b", re.MULTILINE)

CARGO_REQUIRED_SNIPPETS = (
    'bolt-v2 = { path = "../.." }',
    'futures-util = "=0.3.32"',
)

RUN_MANIFEST_CONST_PATTERNS = (
    (
        "binary-oracle registry key constant",
        r'pub\s+const\s+STRATEGY_BINARY_ORACLE_EDGE_TAKER\s*:\s*&str\s*=\s*"binary_oracle_edge_taker"\s*;',
    ),
    (
        "binary-oracle config_toml parameter constant",
        r'pub\s+const\s+STRATEGY_PARAM_CONFIG_TOML\s*:\s*&str\s*=\s*"config_toml"\s*;',
    ),
    (
        "binary-oracle fee_bps parameter constant",
        r'pub\s+const\s+STRATEGY_PARAM_FEE_BPS\s*:\s*&str\s*=\s*"fee_bps"\s*;',
    ),
)

RUN_MANIFEST_STRATEGY_TOKENS = (
    "STRATEGY_BINARY_ORACLE_EDGE_TAKER",
)

RUN_MANIFEST_PARAMETER_TOKENS = (
    "STRATEGY_BINARY_ORACLE_EDGE_TAKER",
    "STRATEGY_PARAM_CONFIG_TOML",
    "STRATEGY_PARAM_FEE_BPS",
)

RUN_MANIFEST_VALIDATE_ARM_PATTERNS = (
    ("config_toml presence", r"\bSTRATEGY_PARAM_CONFIG_TOML\b"),
    ("fee_bps presence", r"\bSTRATEGY_PARAM_FEE_BPS\b"),
    ("fee_bps decimal parse", r"\brust_decimal\s*::\s*Decimal\s*::\s*from_str\s*\("),
    ("non-negative fee guard", r"\bfee_bps\s*<\s*rust_decimal\s*::\s*Decimal\s*::\s*ZERO\b"),
    ("builder TOML parse", r"\btoml\s*::\s*from_str\s*::\s*<\s*toml\s*::\s*Value\s*>\s*\("),
    ("production strategy registry validation", r"\bproduction_strategy_registry\s*\("),
    ("binary-oracle builder kind", r"\bBinaryOracleEdgeTakerBuilder\s*::\s*kind\s*\("),
    ("registry validates builder config", r"\bregistry\s*\.\s*validate\s*\("),
)

RUNNER_ARM_PATTERNS = (
    ("config_toml read from manifest parameters", r"\bparameters\s*\.\s*get\s*\(\s*PARAM_CONFIG_TOML\s*\)"),
    ("config_toml TOML parse", r"\btoml\s*::\s*from_str\s*::\s*<\s*toml\s*::\s*Value\s*>\s*\("),
    ("fee_bps read from manifest parameters", r"\bparameters\s*\.\s*get\s*\(\s*PARAM_FEE_BPS\s*\)"),
    ("fee_bps decimal parse", r"\bDecimal\s*::\s*from_str\s*\("),
    ("non-negative fee guard", r"\bfee_bps\s*>=\s*Decimal\s*::\s*ZERO\b"),
    ("decision evidence writer", r"\bBoltV3DecisionEvidenceWriter\b"),
    ("submit admission state", r"\bBoltV3SubmitAdmissionState\s*::\s*new\s*\("),
    ("manifest fee provider", r"\bManifestFeeProvider\s*\{\s*fee_bps\s*\}"),
    ("strategy build context", r"\bStrategyBuildContext\s*::\s*new\s*\("),
    (
        "manifest venue normalization",
        r"\bVenue\s*::\s*from\s*\(\s*manifest\s*\.\s*venue\s*\.\s*nt_venue\s*\.\s*as_str\s*\(\s*\)\s*\)",
    ),
    ("production strategy registry", r"\bproduction_strategy_registry\s*\("),
    ("registry strategy registration", r"\bregistry\s*\.\s*register_strategy\s*\("),
    (
        "engine trader registration handle",
        r"\bengine\s*\.\s*kernel\s*\(\s*\)\s*\.\s*trader\s*\(\s*\)",
    ),
)


def require_file(root: Path, rel_path: Path, findings: list[str]) -> str:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: file is missing")
        return ""
    return path.read_text(encoding="utf-8")


def require_snippets(rel_path: Path, text: str, snippets: tuple[str, ...], findings: list[str]) -> None:
    for snippet in snippets:
        if snippet not in text:
            findings.append(f"{rel_path}: missing `{snippet}`")


def marker_ids(text: str, marker: str) -> set[str] | None:
    match = re.search(rf"<!--\s*{re.escape(marker)}\s*:\s*(?P<ids>.*?)-->", text, re.DOTALL)
    if match is None:
        return None
    return {part.strip() for part in match.group("ids").replace("\n", " ").split(",") if part.strip()}


def require_marker_ids(rel_path: Path, text: str, marker: str, required_ids: tuple[str, ...], findings: list[str]) -> None:
    ids = marker_ids(text, marker)
    if ids is None:
        findings.append(f"{rel_path}: missing `{marker}` marker")
        return
    for required_id in required_ids:
        if required_id not in ids:
            findings.append(f"{rel_path}: `{marker}` missing `{required_id}`")


def strip_rust_comments(text: str) -> str:
    out: list[str] = []
    i = 0
    block_depth = 0
    state = "code"
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
                out.append(c)
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
            out.append(c)
            if c == "\\":
                if i + 1 < len(text):
                    out.append(text[i + 1])
                    i += 2
                else:
                    i += 1
                continue
            if c == '"':
                state = "code"
            i += 1
            continue
    return "".join(out)


def strip_rust_literals(text: str) -> str:
    out: list[str] = []
    i = 0
    state = "code"
    while i < len(text):
        c = text[i]
        if state == "code":
            if c == '"':
                state = "string"
                out.extend('""')
                i += 1
                continue
            out.append(c)
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


def rust_code_only(text: str) -> str:
    return strip_rust_literals(strip_rust_comments(text))


def require_patterns(
    rel_path: Path,
    text: str,
    patterns: tuple[tuple[str, str], ...],
    findings: list[str],
) -> None:
    for label, pattern in patterns:
        if not re.search(pattern, text, re.DOTALL):
            findings.append(f"{rel_path}: missing real {label}")


def braced_body_after(text: str, pattern: str) -> str | None:
    match = re.search(pattern, text, re.DOTALL)
    if match is None:
        return None
    open_brace = text.find("{", match.end())
    if open_brace == -1:
        return None
    depth = 0
    for index in range(open_brace, len(text)):
        char = text[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return text[open_brace + 1 : index]
    return None


def match_arm_body(text: str, arm_name: str) -> str | None:
    pattern = rf"\b{re.escape(arm_name)}\b\s*=>\s*\{{"
    match = re.search(pattern, text, re.DOTALL)
    if match is None:
        return None
    open_brace = text.rfind("{", 0, match.end())
    if open_brace == -1:
        return None
    depth = 0
    for index in range(open_brace, len(text)):
        char = text[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return text[open_brace + 1 : index]
    return None


def require_tokens(rel_path: Path, body: str, tokens: tuple[str, ...], findings: list[str], label: str) -> None:
    for token in tokens:
        if token not in body:
            findings.append(f"{rel_path}: {label} missing `{token}`")


def verify_run_manifest_wiring(root: Path, findings: list[str]) -> None:
    text = require_file(root, BTE_RUN_MANIFEST, findings)
    if not text:
        return
    without_comments = strip_rust_comments(text)
    code = rust_code_only(text)

    require_patterns(BTE_RUN_MANIFEST, without_comments, RUN_MANIFEST_CONST_PATTERNS, findings)

    strategies_body = braced_body_after(
        code,
        r"pub\s+fn\s+registered_strategies\s*\(",
    )
    if strategies_body is None:
        findings.append(f"{BTE_RUN_MANIFEST}: missing registered_strategies function")
    else:
        require_tokens(
            BTE_RUN_MANIFEST,
            strategies_body,
            RUN_MANIFEST_STRATEGY_TOKENS,
            findings,
            "registered_strategies",
        )

    parameters_body = braced_body_after(
        code,
        r"pub\s+fn\s+registered_strategy_parameters\s*\(",
    )
    if parameters_body is None:
        findings.append(f"{BTE_RUN_MANIFEST}: missing registered_strategy_parameters function")
    else:
        require_tokens(
            BTE_RUN_MANIFEST,
            parameters_body,
            RUN_MANIFEST_PARAMETER_TOKENS,
            findings,
            "registered_strategy_parameters",
        )

    validate_body = braced_body_after(code, r"fn\s+validate_strategy_source\s*\(")
    if validate_body is None:
        findings.append(f"{BTE_RUN_MANIFEST}: missing validate_strategy_source function")
        return
    arm = match_arm_body(validate_body, "STRATEGY_BINARY_ORACLE_EDGE_TAKER")
    if arm is None:
        findings.append(f"{BTE_RUN_MANIFEST}: missing binary_oracle_edge_taker validation arm")
        return
    require_patterns(BTE_RUN_MANIFEST, arm, RUN_MANIFEST_VALIDATE_ARM_PATTERNS, findings)


def verify_runner_wiring(root: Path, findings: list[str]) -> None:
    text = require_file(root, BTE_RUNNER, findings)
    if not text:
        return
    code = rust_code_only(text)
    body = braced_body_after(code, r"fn\s+add_manifest_strategy\s*\(")
    if body is None:
        findings.append(f"{BTE_RUNNER}: missing add_manifest_strategy runner function")
        return
    arm = match_arm_body(body, "STRATEGY_BINARY_ORACLE_EDGE_TAKER")
    if arm is None:
        findings.append(f"{BTE_RUNNER}: missing binary_oracle_edge_taker runner arm")
        return
    require_patterns(BTE_RUNNER, arm, RUNNER_ARM_PATTERNS, findings)


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    plan_text = require_file(root, PLAN_PATH, findings)
    spec_text = require_file(root, SPEC_PATH, findings)
    tasks_text = require_file(root, TASKS_PATH, findings)

    require_marker_ids(PLAN_PATH, plan_text, PREREQUISITE_MARKER, PREREQUISITE_REQUIRED_IDS, findings)
    require_marker_ids(SPEC_PATH, spec_text, PREREQUISITE_MARKER, PREREQUISITE_REQUIRED_IDS, findings)

    if tasks_text and not CHECKED_RA016.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-016 must be checked once the prerequisite is documented")

    cargo_text = require_file(root, BTE_CARGO_TOML, findings)
    require_snippets(BTE_CARGO_TOML, cargo_text, CARGO_REQUIRED_SNIPPETS, findings)
    verify_run_manifest_wiring(root, findings)
    verify_runner_wiring(root, findings)

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA BTE phase prerequisite violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA BTE phase prerequisite passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
