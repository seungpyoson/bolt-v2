#!/usr/bin/env python3
"""Reject hardcoded Bolt-v3 reference-policy fixture literals in Rust tests."""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
REFERENCE_STREAM_FIXTURE_GLOBS = (
    "tests/fixtures/bolt_v3_reference_policy/*_stream.toml",
    "tests/fixtures/bolt_v3_reference_producer/*_stream.toml",
)
ROOT_FIXTURE_GLOB = "tests/fixtures/bolt_v3*/root*.toml"
ENFORCED_TEST_FILES = (
    "tests/bolt_v3_adapter_mapping.rs",
    "tests/bolt_v3_reference_actor_registration.rs",
    "tests/bolt_v3_reference_delivery.rs",
    "tests/bolt_v3_reference_policy.rs",
    "tests/bolt_v3_reference_producer.rs",
    "tests/bolt_v3_scale_process.rs",
    "tests/bolt_v3_strategy_registration.rs",
)
ENFORCED_SOURCE_FILES = ("src/bolt_v3_validate.rs",)
STRING_PATTERN = re.compile(r'"(?:\\.|[^"\\])*"')
REFERENCE_STREAM_PARAMETER_LITERAL_PATTERN = re.compile(r'"reference_stream_id"')
AUTO_DISABLE_REASON_LITERAL_PATTERN = re.compile(
    r'"auto-disabled after [^"]* without a fresh reference update"'
)
INLINE_REFERENCE_POLICY_SCENARIO_VALUE_PATTERN = re.compile(
    r"\blet\s+(?:oracle_price|orderbook_bid|orderbook_ask|observed_price)\s*=\s*[0-9][^;]*;"
)
REFERENCE_DELIVERY_OBSERVATION_PRICE_PATTERN = re.compile(
    r"(?:\bprice:\s*|\bSome\()\d[\d_]*(?:\.\d+)?"
)
REFERENCE_DELIVERY_TIMESTAMP_CONVERSION_PATTERN = re.compile(
    r"\.saturating_mul\(\s*1_000_000\s*\)"
)
INLINE_REFERENCE_POLICY_REASON_LITERAL_PATTERN = re.compile(r'"test disables \{\}"')


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    message: str
    excerpt: str

    def render(self) -> str:
        return f"FAIL: {self.path}:{self.line}: {self.message}: {self.excerpt}"


def line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def string_value(literal: str) -> str:
    return bytes(literal[1:-1], "utf-8").decode("unicode_escape")


def add_stream_literals(literals: set[str], stream_id: str, stream: dict[str, Any]) -> None:
    if stream_id:
        literals.add(stream_id)
    publish_topic = stream.get("publish_topic")
    if isinstance(publish_topic, str):
        literals.add(publish_topic)
    inputs = stream.get("inputs", [])
    if not isinstance(inputs, list):
        return
    for input_block in inputs:
        if not isinstance(input_block, dict):
            continue
        for key in ("source_id", "instrument_id"):
            value = input_block.get(key)
            if isinstance(value, str):
                literals.add(value)


def reference_policy_fixture_literals(root: Path) -> set[str]:
    literals: set[str] = set()
    for glob in REFERENCE_STREAM_FIXTURE_GLOBS:
        for path in sorted(root.glob(glob)):
            if not path.is_file():
                continue
            stream_id = path.name.removesuffix("_stream.toml")
            add_stream_literals(
                literals, stream_id, tomllib.loads(path.read_text(encoding="utf-8"))
            )
    for path in sorted(root.glob(ROOT_FIXTURE_GLOB)):
        if not path.is_file():
            continue
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        streams = data.get("reference_streams", {})
        if not isinstance(streams, dict):
            continue
        for stream_id, stream in streams.items():
            if isinstance(stream, dict):
                add_stream_literals(literals, str(stream_id), stream)
    return literals


def scan_file(root: Path, path: Path, fixture_literals: set[str]) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []
    for match in REFERENCE_STREAM_PARAMETER_LITERAL_PATTERN.finditer(text):
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="reference stream parameter-key literal; use REFERENCE_STREAM_ID_PARAMETER",
                excerpt=match.group(0),
            )
        )
    for match in AUTO_DISABLE_REASON_LITERAL_PATTERN.finditer(text):
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="reference auto-disable reason literal; derive with reference_auto_disable_reason",
                excerpt=match.group(0),
            )
        )
    if rel == "tests/bolt_v3_reference_policy.rs":
        for match in INLINE_REFERENCE_POLICY_SCENARIO_VALUE_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="reference-policy scenario value literal; load from test fixture",
                    excerpt=match.group(0),
                )
            )
        for match in INLINE_REFERENCE_POLICY_REASON_LITERAL_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="reference-policy manual disable reason literal; load from test fixture",
                    excerpt=match.group(0),
                )
            )
    if rel == "tests/bolt_v3_reference_delivery.rs":
        for match in REFERENCE_DELIVERY_OBSERVATION_PRICE_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="reference-delivery observation price literal; load from test fixture",
                    excerpt=match.group(0),
                )
            )
        for match in REFERENCE_DELIVERY_TIMESTAMP_CONVERSION_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "reference-delivery timestamp conversion literal; use Duration-based helper"
                    ),
                    excerpt=match.group(0),
                )
            )
    for match in STRING_PATTERN.finditer(text):
        literal = match.group(0)
        value = string_value(literal)
        if value not in fixture_literals:
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="reference-policy fixture literal; derive from loaded TOML fixture instead",
                excerpt=literal,
            )
        )
    return findings


def scan_root(root: Path) -> list[Finding]:
    fixture_literals = reference_policy_fixture_literals(root)
    findings: list[Finding] = []
    for relative_path in ENFORCED_TEST_FILES + ENFORCED_SOURCE_FILES:
        path = root / relative_path
        if path.is_file():
            findings.extend(scan_file(root, path, fixture_literals))
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 reference-policy fixture-literal verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
