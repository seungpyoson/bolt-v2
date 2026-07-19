#!/usr/bin/env python3
"""Validate the #1354 novelty registry and its deterministic Rust projection."""

from __future__ import annotations

import argparse
import pathlib
import re
import sys
import tomllib
from dataclasses import dataclass


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
REGISTRY_PATH = pathlib.Path("config/evidence-novelty.toml")
GENERATED_PATH = pathlib.Path("src/bolt_v3_evidence_novelty/generated.rs")
PRODUCER_PATH = pathlib.Path("src/strategies/binary_oracle_edge_taker/mod.rs")


@dataclass(frozen=True)
class Allocation:
    name: str
    id_start: int
    id_end_exclusive: int


@dataclass(frozen=True)
class State:
    rust_variant: str
    owner: str
    producer_kind: str
    semantic_state: str
    allocation: str
    id: int


@dataclass(frozen=True)
class Registry:
    family_name: str
    family_capacity: int
    allocations: tuple[Allocation, ...]
    states: tuple[State, ...]


def _strict_keys(value: object, expected: set[str], context: str) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != expected:
        raise ValueError(f"{context} must contain exactly {sorted(expected)}")
    return value


def load_registry(path: pathlib.Path) -> Registry:
    document = _strict_keys(
        tomllib.loads(path.read_text(encoding="utf-8")),
        {"schema_version", "family", "allocation", "state"},
        "registry",
    )
    if document["schema_version"] != 1:
        raise ValueError("registry schema_version must be 1")

    family = _strict_keys(document["family"], {"name", "capacity"}, "family")
    family_name = family["name"]
    capacity = family["capacity"]
    if not isinstance(family_name, str) or not re.fullmatch(r"[a-z][a-z0-9_]*", family_name):
        raise ValueError("family.name must be snake_case")
    if type(capacity) is not int or not 1 <= capacity <= 65_536:
        raise ValueError("family.capacity must be an integer in 1..=65536")

    raw_allocations = document["allocation"]
    if not isinstance(raw_allocations, list) or not raw_allocations:
        raise ValueError("registry requires allocation rows")
    allocations: list[Allocation] = []
    cursor = 0
    for index, value in enumerate(raw_allocations):
        row = Allocation(**_strict_keys(value, {"name", "id_start", "id_end_exclusive"}, f"allocation[{index}]"))
        if not re.fullmatch(r"[a-z][a-z0-9_]*", row.name):
            raise ValueError(f"allocation[{index}].name must be snake_case")
        if type(row.id_start) is not int or type(row.id_end_exclusive) is not int:
            raise ValueError(f"allocation[{index}] bounds must be integers")
        if row.id_start != cursor or row.id_end_exclusive <= row.id_start:
            raise ValueError("allocations must be ordered, contiguous, and non-empty")
        cursor = row.id_end_exclusive
        allocations.append(row)
    if cursor != capacity:
        raise ValueError("allocations must cover the complete family capacity")
    allocation_names = [row.name for row in allocations]
    if len(set(allocation_names)) != len(allocation_names):
        raise ValueError("allocation names must be unique")

    raw_states = document["state"]
    if not isinstance(raw_states, list) or not raw_states:
        raise ValueError("registry requires state rows")
    states: list[State] = []
    state_keys = {"rust_variant", "owner", "producer_kind", "semantic_state", "allocation", "id"}
    for index, value in enumerate(raw_states):
        row = State(**_strict_keys(value, state_keys, f"state[{index}]"))
        if not re.fullmatch(r"[A-Z][A-Za-z0-9]*", row.rust_variant):
            raise ValueError(f"state[{index}].rust_variant must be UpperCamelCase")
        if not re.fullmatch(r"[A-Z][A-Za-z0-9]*", row.owner):
            raise ValueError(f"state[{index}].owner must be UpperCamelCase")
        if not re.fullmatch(r"[a-z][a-z0-9_]*", row.producer_kind):
            raise ValueError(f"state[{index}].producer_kind must be snake_case")
        if not re.fullmatch(r"[a-z][a-z0-9_.]*", row.semantic_state):
            raise ValueError(f"state[{index}].semantic_state must be dotted snake_case")
        if not row.semantic_state.startswith(f"{row.producer_kind}."):
            raise ValueError(f"state[{index}] semantic_state must belong to producer_kind")
        if row.allocation not in allocation_names:
            raise ValueError(f"state[{index}] references an unknown allocation")
        allocation = allocations[allocation_names.index(row.allocation)]
        if type(row.id) is not int or not allocation.id_start <= row.id < allocation.id_end_exclusive:
            raise ValueError(f"state[{index}].id is outside its allocation")
        states.append(row)

    for label, values in {
        "rust_variant": [row.rust_variant for row in states],
        "semantic_state": [(row.producer_kind, row.semantic_state) for row in states],
        "id": [row.id for row in states],
    }.items():
        if len(set(values)) != len(values):
            raise ValueError(f"state {label} values must be unique")
    if [row.id for row in states] != sorted(row.id for row in states):
        raise ValueError("state rows must be ordered by id")
    owners_by_producer: dict[str, set[str]] = {}
    for row in states:
        owners_by_producer.setdefault(row.producer_kind, set()).add(row.owner)
    if any(len(owners) != 1 for owners in owners_by_producer.values()):
        raise ValueError("each producer_kind must have exactly one owner")

    return Registry(family_name, capacity, tuple(allocations), tuple(states))


def render_registry(registry: Registry) -> str:
    owners = tuple(dict.fromkeys(row.owner for row in registry.states))
    lines = [
        "// @generated by scripts/verify_bolt_v3_evidence_novelty.py from",
        "// config/evidence-novelty.toml. Do not edit.",
        "",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]",
        "pub enum EvidenceStateOwner {",
        *(f"    {owner}," for owner in owners),
        "}",
        "",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]",
        "#[repr(u16)]",
        "pub enum EvidenceCanonicalState {",
        *(f"    {row.rust_variant} = {row.id}," for row in registry.states),
        "}",
        "",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "pub struct EvidenceStateRegistration {",
        "    pub state: EvidenceCanonicalState,",
        "    pub owner: EvidenceStateOwner,",
        "    pub family: &'static str,",
        "    pub producer_kind: &'static str,",
        "    pub semantic_state: &'static str,",
        "    pub id: usize,",
        "}",
        "",
        f"pub const EVIDENCE_NOVELTY_FAMILY_CAPACITY: usize = {registry.family_capacity};",
        f"pub const EVIDENCE_NOVELTY_WORD_COUNT: usize = {(registry.family_capacity + 63) // 64};",
        "",
        "pub const EVIDENCE_STATE_REGISTRATIONS: &[EvidenceStateRegistration] = &[",
    ]
    for row in registry.states:
        lines.extend([
            "    EvidenceStateRegistration {",
            f"        state: EvidenceCanonicalState::{row.rust_variant},",
            f"        owner: EvidenceStateOwner::{row.owner},",
            f'        family: "{registry.family_name}",',
            f'        producer_kind: "{row.producer_kind}",',
            f'        semantic_state: "{row.semantic_state}",',
            f"        id: {row.id},",
            "    },",
        ])
    lines.extend(
        [
            "];",
            "",
            "pub const fn canonical_state_registration(",
            "    state: EvidenceCanonicalState,",
            ") -> &'static EvidenceStateRegistration {",
            "    match state {",
        ]
    )
    for index, row in enumerate(registry.states):
        lines.append(f"        EvidenceCanonicalState::{row.rust_variant} => &EVIDENCE_STATE_REGISTRATIONS[{index}],")
    lines.extend(
        [
            "    }",
            "}",
            "",
            "pub const fn evidence_state_registration_by_id(",
            "    id: usize,",
            ") -> Option<&'static EvidenceStateRegistration> {",
            "    match id {",
        ]
    )
    for index, row in enumerate(registry.states):
        lines.append(f"        {row.id} => Some(&EVIDENCE_STATE_REGISTRATIONS[{index}]),")
    lines.extend(["        _ => None,", "    }", "}", ""])
    return "\n".join(lines)


def repository_errors(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    try:
        registry = load_registry(root / REGISTRY_PATH)
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError, TypeError, ValueError) as error:
        return [str(error)]
    expected = render_registry(registry)
    try:
        actual = (root / GENERATED_PATH).read_text(encoding="utf-8")
    except OSError as error:
        errors.append(str(error))
    else:
        if actual != expected:
            errors.append("generated novelty Rust is stale; run verifier with --write")

    try:
        producer = (root / PRODUCER_PATH).read_text(encoding="utf-8")
    except OSError as error:
        errors.append(str(error))
    else:
        referenced = set(re.findall(r"EvidenceCanonicalState::([A-Z][A-Za-z0-9]*)", producer))
        registered = {row.rust_variant for row in registry.states}
        if referenced != registered:
            errors.append(
                "producer canonical-state references must exactly match registry: "
                f"missing={sorted(registered - referenced)} unknown={sorted(referenced - registered)}"
            )
    return errors


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--write", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.write:
        registry = load_registry(REPO_ROOT / REGISTRY_PATH)
        output = REPO_ROOT / GENERATED_PATH
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(render_registry(registry), encoding="utf-8")
        return 0
    errors = repository_errors(REPO_ROOT)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: evidence novelty TOML authority and generated Rust agree.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
