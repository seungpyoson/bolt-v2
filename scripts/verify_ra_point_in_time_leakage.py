#!/usr/bin/env python3
"""Verify Research Analytics point-in-time feature join fixtures."""

from __future__ import annotations

import argparse
import sys
import tomllib
from collections import defaultdict
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_FIXTURE_DIR = (
    REPO_ROOT
    / "specs"
    / "023-nt-research-analytics-platform"
    / "2-research-analytics"
    / "leakage-fixtures"
)

DATASET_HASH_FIELDS = ("source_hash", "query_hash", "config_hash")
RULE_STRING_FIELDS = (
    "observation_event_time",
    "observation_as_of_time",
    "feature_event_time",
    "feature_availability_time",
    "feature_source_hash",
)
EXPECTED_VALUES = {"pass", "fail"}


def load_fixture(path: Path) -> dict[str, Any]:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def valid_sha256_ref(value: Any) -> bool:
    return isinstance(value, str) and value.startswith("sha256:") and len(value) > len(
        "sha256:"
    )


def add_required_table_findings(
    findings: list[str], path: Path, data: dict[str, Any], table_name: str
) -> dict[str, Any]:
    value = data.get(table_name)
    if not isinstance(value, dict):
        findings.append(f"{path.name}: missing [{table_name}] table")
        return {}
    return value


def required_string(
    findings: list[str], path: Path, table_name: str, table: dict[str, Any], key: str
) -> str | None:
    value = table.get(key)
    if not isinstance(value, str) or not value:
        findings.append(f"{path.name}: {table_name}.{key} must be a non-empty string")
        return None
    return value


def required_int(
    findings: list[str], path: Path, label: str, row: dict[str, Any], key: str
) -> int | None:
    value = row.get(key)
    if not isinstance(value, int):
        findings.append(f"{path.name}: {label}.{key} must be an integer timestamp")
        return None
    return value


def case_expected(path: Path, data: dict[str, Any]) -> str | None:
    case = data.get("case")
    if not isinstance(case, dict):
        return None
    expected = case.get("expected")
    if not isinstance(expected, str) or expected not in EXPECTED_VALUES:
        return None
    return expected


def validate_case_metadata(path: Path, data: dict[str, Any], findings: list[str]) -> None:
    expected = case_expected(path, data)
    if expected is None:
        findings.append(f"{path.name}: case.expected must be pass or fail")

    dataset = add_required_table_findings(findings, path, data, "dataset")
    for key in DATASET_HASH_FIELDS:
        if not valid_sha256_ref(dataset.get(key)):
            findings.append(f"{path.name}: dataset.{key} must be a sha256 reference")


def rules_from_fixture(path: Path, data: dict[str, Any], findings: list[str]) -> dict[str, Any]:
    rules = add_required_table_findings(findings, path, data, "rules")
    join_keys = rules.get("join_keys")
    if not isinstance(join_keys, list) or not join_keys:
        findings.append(f"{path.name}: rules.join_keys must be a non-empty list")
    elif not all(isinstance(key, str) and key for key in join_keys):
        findings.append(f"{path.name}: rules.join_keys entries must be non-empty strings")

    for key in RULE_STRING_FIELDS:
        required_string(findings, path, "rules", rules, key)

    max_staleness = rules.get("max_staleness_nanos")
    if not isinstance(max_staleness, int) or max_staleness < 0:
        findings.append(f"{path.name}: rules.max_staleness_nanos must be a non-negative integer")
    return rules


def rows_from_fixture(
    path: Path, data: dict[str, Any], table_name: str, findings: list[str]
) -> list[dict[str, Any]]:
    rows = data.get(table_name)
    if not isinstance(rows, list) or not rows:
        findings.append(f"{path.name}: [[{table_name}]] must contain at least one row")
        return []
    typed_rows: list[dict[str, Any]] = []
    for index, row in enumerate(rows, start=1):
        if not isinstance(row, dict):
            findings.append(f"{path.name}: {table_name}[{index}] must be a table")
            continue
        typed_rows.append(row)
    return typed_rows


def join_tuple(row: dict[str, Any], join_keys: list[str]) -> tuple[Any, ...]:
    return tuple(row.get(key) for key in join_keys)


def validate_fixture(path: Path) -> list[str]:
    data = load_fixture(path)
    findings: list[str] = []
    validate_case_metadata(path, data, findings)
    rules = rules_from_fixture(path, data, findings)

    join_keys = rules.get("join_keys")
    if not isinstance(join_keys, list) or not all(isinstance(key, str) for key in join_keys):
        return findings

    obs_event_key = rules.get("observation_event_time")
    obs_asof_key = rules.get("observation_as_of_time")
    feature_event_key = rules.get("feature_event_time")
    feature_available_key = rules.get("feature_availability_time")
    feature_hash_key = rules.get("feature_source_hash")
    max_staleness = rules.get("max_staleness_nanos")
    if not all(
        isinstance(value, str)
        for value in (
            obs_event_key,
            obs_asof_key,
            feature_event_key,
            feature_available_key,
            feature_hash_key,
        )
    ) or not isinstance(max_staleness, int):
        return findings

    observations = rows_from_fixture(path, data, "observations", findings)
    features = rows_from_fixture(path, data, "features", findings)

    observations_by_key: dict[tuple[Any, ...], list[dict[str, Any]]] = defaultdict(list)
    for index, observation in enumerate(observations, start=1):
        missing_keys = [key for key in join_keys if key not in observation]
        if missing_keys:
            findings.append(
                f"{path.name}: observations[{index}] missing join key(s) {', '.join(missing_keys)}"
            )
            continue
        obs_event = required_int(findings, path, f"observations[{index}]", observation, obs_event_key)
        obs_asof = required_int(findings, path, f"observations[{index}]", observation, obs_asof_key)
        if obs_event is not None and obs_asof is not None and obs_asof < obs_event:
            findings.append(
                f"{path.name}: observations[{index}].{obs_asof_key} precedes {obs_event_key}"
            )
        observations_by_key[join_tuple(observation, join_keys)].append(observation)

    for index, feature in enumerate(features, start=1):
        missing_keys = [key for key in join_keys if key not in feature]
        if missing_keys:
            findings.append(
                f"{path.name}: features[{index}] missing join key(s) {', '.join(missing_keys)}"
            )
            continue
        if not valid_sha256_ref(feature.get(feature_hash_key)):
            findings.append(f"{path.name}: features[{index}].{feature_hash_key} must be a sha256 reference")

        feature_event = required_int(findings, path, f"features[{index}]", feature, feature_event_key)
        feature_available = required_int(
            findings, path, f"features[{index}]", feature, feature_available_key
        )
        matching_observations = observations_by_key.get(join_tuple(feature, join_keys), [])
        if not matching_observations:
            findings.append(f"{path.name}: features[{index}] has no matching observation")
            continue
        if feature_event is None or feature_available is None:
            continue

        for observation in matching_observations:
            obs_event = observation.get(obs_event_key)
            obs_asof = observation.get(obs_asof_key)
            if not isinstance(obs_event, int) or not isinstance(obs_asof, int):
                continue
            if feature_event > obs_event:
                findings.append(
                    f"{path.name}: features[{index}].{feature_event_key} is after observation {obs_event_key}"
                )
            if feature_available > obs_asof:
                findings.append(
                    f"{path.name}: features[{index}].{feature_available_key} is after observation {obs_asof_key}"
                )
            elif obs_asof - feature_available > max_staleness:
                findings.append(
                    f"{path.name}: features[{index}] exceeds rules.max_staleness_nanos"
                )

    return findings


def validate_fixture_dir(fixture_dir: Path) -> list[str]:
    fixture_paths = sorted(fixture_dir.glob("*.toml"))
    if not fixture_paths:
        return [f"{fixture_dir}: no point-in-time leakage fixture TOMLs found"]

    failures: list[str] = []
    for path in fixture_paths:
        data = load_fixture(path)
        expected = case_expected(path, data)
        findings = validate_fixture(path)
        if expected == "fail":
            if not findings:
                failures.append(f"{path.name}: expected fail but passed")
        else:
            if findings:
                failures.append(f"{path.name}: expected pass but failed")
                failures.extend(f"  {finding}" for finding in findings)
    return failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture-dir", type=Path, default=DEFAULT_FIXTURE_DIR)
    args = parser.parse_args(argv)

    failures = validate_fixture_dir(args.fixture_dir)
    if failures:
        print("FAIL: RA point-in-time leakage fixture violations:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1
    print("OK: RA point-in-time leakage fixtures passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
