#!/usr/bin/env python3
"""Fail-closed verifier for the #1354 evidence producer/reader registry."""

from __future__ import annotations

import argparse
import pathlib
import re
import sys
import tomllib
from typing import Any


ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_REGISTRY = ROOT / "ci/bolt-v3-evidence-registry.toml"

ROOT_KEYS = {
    "schema_version",
    "source_module",
    "identity_module",
    "closed_families",
    "allowed_classifications",
    "allowed_suppression",
    "handler_sweep_roots",
    "non_evidence_per_tick_appenders",
    "family",
    "reader",
    "producer",
}
FAMILY_KEYS = {"name", "capacity", "allocations"}
ALLOCATION_KEYS = {"name", "start", "end"}
READER_KEYS = {"name", "record_kinds", "recovery_role"}
PRODUCER_KEYS = {
    "name",
    "method",
    "record_kind",
    "gate_id",
    "family",
    "state_id",
    "allocation",
    "classification",
    "handler_reachability",
    "source_paths",
    "named_readers",
    "repeat_semantics",
    "dedupe_key_evidence",
    "recovery_bearing",
    "suppression",
    "owner_decision_required",
}

FROZEN_FAMILIES = {
    "risk": (
        64,
        [
            ("admission_entry", 0, 8),
            ("order_prepare_submit_fill_terminal", 8, 24),
            ("position_exposure", 24, 32),
            ("exit_cancel_replacement", 32, 48),
            ("settlement_redemption", 48, 56),
            ("reconciliation_dependency", 56, 62),
            ("terminal_integrity", 62, 64),
        ],
    ),
    "market": (
        256,
        [
            ("discovery_identity", 0, 32),
            ("lifecycle_rollover", 32, 80),
            ("subscription_book", 80, 144),
            ("strategy_input_pricing_blocker", 144, 208),
            ("dependency_health", 208, 240),
            ("terminal_closed_window_skip", 240, 256),
        ],
    ),
    "system": (
        64,
        [
            ("startup_recovery", 0, 16),
            ("storage_archive", 16, 32),
            ("authentication_network_provider", 32, 48),
            ("capacity_host", 48, 60),
            ("integrity_operator", 60, 64),
        ],
    ),
}

EXPECTED_CLASSIFICATIONS = {
    "event-keyed",
    "state-observation",
    "already-deduped",
    "no-named-reader",
}
EXPECTED_SUPPRESSION = {"unsuppressed", "finite-episode"}
EXPECTED_HANDLER_CLASSES = {"quote", "book", "timer", "index-price", "startup"}
APPEND_PATTERNS = (
    re.compile(r"\.append\s*\(\s*true\s*\)"),
    re.compile(r"OpenOptions"),
    re.compile(r"\bFile::create\s*\("),
    re.compile(r"\b(?:fs|std::fs)::write\s*\("),
    re.compile(r"\.write_all\s*\("),
)


class RegistryError(ValueError):
    pass


def require_exact_keys(row: dict[str, Any], allowed: set[str], context: str) -> None:
    unknown = set(row) - allowed
    missing = allowed - set(row)
    if unknown:
        raise RegistryError(f"{context}: unknown registry keys {sorted(unknown)}")
    if missing:
        raise RegistryError(f"{context}: missing registry keys {sorted(missing)}")


def load_registry(path: pathlib.Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            data = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise RegistryError(f"cannot load {path}: {error}") from error
    require_exact_keys(data, ROOT_KEYS, "registry")
    if data["schema_version"] != 1:
        raise RegistryError("registry: schema_version must equal 1")
    return data


def validate_families(data: dict[str, Any]) -> dict[str, list[tuple[str, int, int]]]:
    rows = data["family"]
    if not isinstance(rows, list) or len(rows) != len(FROZEN_FAMILIES):
        raise RegistryError("registry: family rows must define exactly risk, market, and system")
    observed: dict[str, list[tuple[str, int, int]]] = {}
    for index, row in enumerate(rows):
        require_exact_keys(row, FAMILY_KEYS, f"family[{index}]")
        name = row["name"]
        if name in observed:
            raise RegistryError(f"family[{index}]: duplicate family {name!r}")
        expected = FROZEN_FAMILIES.get(name)
        if expected is None:
            raise RegistryError(f"family[{index}]: unknown family {name!r}")
        capacity, expected_allocations = expected
        if row["capacity"] != capacity:
            raise RegistryError(f"family {name}: capacity must equal frozen value {capacity}")
        allocations: list[tuple[str, int, int]] = []
        for allocation_index, allocation in enumerate(row["allocations"]):
            require_exact_keys(
                allocation,
                ALLOCATION_KEYS,
                f"family {name} allocation[{allocation_index}]",
            )
            allocations.append((allocation["name"], allocation["start"], allocation["end"]))
        if allocations != expected_allocations:
            raise RegistryError(f"family {name}: allocation differs from frozen contract")
        observed[name] = allocations
    if set(observed) != set(FROZEN_FAMILIES):
        raise RegistryError("registry: closed family set mismatch")
    return observed


def source_trait_methods(source: str) -> set[str]:
    match = re.search(
        r"pub trait BoltV3DecisionEvidenceWriter.*?\n}\n\n/// Risk direction",
        source,
        flags=re.DOTALL,
    )
    if match is None:
        raise RegistryError("source: cannot locate BoltV3DecisionEvidenceWriter trait")
    return set(re.findall(r"\bfn (record_[a-z_]+)\s*\(", match.group(0)))


def source_public_readers(source: str) -> set[str]:
    return set(re.findall(r"^pub fn (read_[a-z_]+)\s*\(", source, flags=re.MULTILINE))


def validate_readers(data: dict[str, Any], source: str) -> set[str]:
    rows = data["reader"]
    names: set[str] = set()
    for index, row in enumerate(rows):
        require_exact_keys(row, READER_KEYS, f"reader[{index}]")
        name = row["name"]
        if name in names:
            raise RegistryError(f"reader[{index}]: duplicate reader {name!r}")
        if not row["record_kinds"]:
            raise RegistryError(f"reader {name}: record_kinds must not be empty")
        if not isinstance(row["recovery_role"], bool):
            raise RegistryError(f"reader {name}: recovery_role must be boolean")
        names.add(name)
    actual = source_public_readers(source)
    if names != actual:
        raise RegistryError(
            f"reader census mismatch: missing={sorted(actual - names)} unknown={sorted(names - actual)}"
        )
    return names


def allocation_for(
    allocations: dict[str, list[tuple[str, int, int]]], family: str, state_id: int
) -> str | None:
    for name, start, end in allocations[family]:
        if start <= state_id < end:
            return name
    return None


def validate_producers(
    data: dict[str, Any],
    source: str,
    allocations: dict[str, list[tuple[str, int, int]]],
    readers: set[str],
) -> None:
    rows = data["producer"]
    names: set[str] = set()
    family_ids: set[tuple[str, int]] = set()
    methods: dict[str, int] = {}
    for index, row in enumerate(rows):
        require_exact_keys(row, PRODUCER_KEYS, f"producer[{index}]")
        name = row["name"]
        if name in names:
            raise RegistryError(f"producer[{index}]: duplicate name {name!r}")
        names.add(name)
        family = row["family"]
        if family not in allocations:
            raise RegistryError(f"producer {name}: unknown family {family!r}")
        state_id = row["state_id"]
        if not isinstance(state_id, int) or isinstance(state_id, bool):
            raise RegistryError(f"producer {name}: state_id must be an integer")
        family_id = (family, state_id)
        if family_id in family_ids:
            raise RegistryError(f"producer {name}: duplicate family/id {family}/{state_id}")
        family_ids.add(family_id)
        allocation = allocation_for(allocations, family, state_id)
        if allocation is None:
            raise RegistryError(f"producer {name}: unassigned family/id {family}/{state_id}")
        if row["allocation"] != allocation:
            raise RegistryError(
                f"producer {name}: allocation {row['allocation']!r} does not own {family}/{state_id}"
            )
        if row["classification"] not in EXPECTED_CLASSIFICATIONS:
            raise RegistryError(f"producer {name}: unknown classification")
        if row["suppression"] not in EXPECTED_SUPPRESSION:
            raise RegistryError(f"producer {name}: unknown suppression")
        if row["recovery_bearing"] and row["suppression"] != "unsuppressed":
            raise RegistryError(f"recovery-bearing producer {name} must remain unsuppressed")
        if row["classification"] == "no-named-reader" and not row["owner_decision_required"]:
            raise RegistryError(f"producer {name}: no-named-reader row requires owner decision")
        if set(row["named_readers"]) - readers:
            raise RegistryError(f"producer {name}: names an unknown reader")
        if not row["handler_reachability"] or set(row["handler_reachability"]) - EXPECTED_HANDLER_CLASSES:
            raise RegistryError(f"producer {name}: invalid handler reachability")
        if not row["source_paths"] or not row["repeat_semantics"] or not row["dedupe_key_evidence"]:
            raise RegistryError(f"producer {name}: incomplete census evidence")
        methods[row["method"]] = methods.get(row["method"], 0) + 1

    actual_methods = source_trait_methods(source)
    registered_methods = set(methods)
    if registered_methods != actual_methods:
        raise RegistryError(
            "producer census mismatch: "
            f"missing={sorted(actual_methods - registered_methods)} "
            f"unknown={sorted(registered_methods - actual_methods)}"
        )
    for method, count in methods.items():
        expected_count = 2 if method == "record_strategy_input_snapshot" else 1
        if count != expected_count:
            raise RegistryError(
                f"producer method {method}: expected {expected_count} structural row(s), found {count}"
            )


def validate_header_contract(data: dict[str, Any]) -> None:
    if data["closed_families"] != ["risk", "market", "system"]:
        raise RegistryError("registry: closed_families must be risk, market, system in order")
    if set(data["allowed_classifications"]) != EXPECTED_CLASSIFICATIONS:
        raise RegistryError("registry: allowed_classifications mismatch")
    if set(data["allowed_suppression"]) != EXPECTED_SUPPRESSION:
        raise RegistryError("registry: allowed_suppression mismatch")
    if data["non_evidence_per_tick_appenders"]:
        raise RegistryError("registry: non-evidence per-tick appenders require an explicit owner decision")


def validate_handler_append_sweep(data: dict[str, Any]) -> None:
    findings: list[str] = []
    for relative_root in data["handler_sweep_roots"]:
        root = ROOT / relative_root
        paths = [root] if root.is_file() else sorted(root.rglob("*.rs"))
        for path in paths:
            if "tests" in path.relative_to(ROOT).parts:
                continue
            text = path.read_text(encoding="utf-8")
            for pattern in APPEND_PATTERNS:
                if pattern.search(text):
                    findings.append(f"{path.relative_to(ROOT)}:{pattern.pattern}")
    if findings:
        raise RegistryError(
            "non-evidence disk appender found in strategy/live-node handler sweep: "
            + ", ".join(findings)
        )


def validate_identity_type(data: dict[str, Any]) -> None:
    path = ROOT / data["identity_module"]
    source = path.read_text(encoding="utf-8")
    identity_types = (
        "EvidenceEpisodeId",
        "EvidenceMarketIdentity",
        "EvidenceOutcomeIdentity",
    )
    fields: list[str] = []
    episode_body = ""
    for type_name in identity_types:
        match = re.search(
            rf"pub struct {type_name}\s*\{{(?P<body>.*?)\n\}}", source, re.DOTALL
        )
        if match is None:
            raise RegistryError(f"identity: cannot locate {type_name}")
        body = match.group("body")
        fields.extend(re.findall(r"^\s*([a-z][a-z0-9_]*)\s*:", body, re.MULTILINE))
        if type_name == "EvidenceEpisodeId":
            episode_body = body.lower()
    forbidden = (
        "price",
        "timestamp",
        "_ts",
        "_ms",
        "_ns",
        "slug",
        "window",
        "diagnostic",
        "transient",
        "flag",
        "retry",
        "schema",
        "config",
        "deploy",
        "age",
        "counter",
        "ordinal",
        "version",
        "digest",
    )
    present = sorted(
        field for field in fields if any(token in field for token in forbidden)
    )
    if present:
        raise RegistryError(f"identity: forbidden volatile fields present {present}")
    required = {
        "logical_strategy_id",
        "logical_target_id",
        "logical_venue_id",
        "market",
    }
    missing = sorted(token for token in required if token not in episode_body)
    if missing:
        raise RegistryError(f"identity: required stable fields missing {missing}")


def verify(path: pathlib.Path) -> None:
    data = load_registry(path)
    validate_header_contract(data)
    source_path = ROOT / data["source_module"]
    source = source_path.read_text(encoding="utf-8")
    allocations = validate_families(data)
    readers = validate_readers(data, source)
    validate_producers(data, source, allocations, readers)
    validate_handler_append_sweep(data)
    validate_identity_type(data)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry", type=pathlib.Path, default=DEFAULT_REGISTRY)
    args = parser.parse_args()
    try:
        verify(args.registry)
    except (RegistryError, OSError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: bolt-v3 evidence registry verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
