#!/usr/bin/env python3
"""Validate the #1354 novelty registry and its deterministic Rust projection."""

from __future__ import annotations

import argparse
import ast
import pathlib
import re
import sys
import tomllib
from dataclasses import dataclass

from rust_source_scanner import strip_rust_comments_and_literals


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
REGISTRY_PATH = pathlib.Path("config/evidence-novelty.toml")
GENERATED_PATH = pathlib.Path("src/bolt_v3_evidence_novelty/generated.rs")
PRODUCER_PATH = pathlib.Path("src/strategies/binary_oracle_edge_taker/mod.rs")
ENTRY_DECISION_PATH = pathlib.Path("src/strategies/binary_oracle_edge_taker/entry_decision.rs")
DECISION_EVIDENCE_PATH = pathlib.Path("src/bolt_v3_decision_evidence.rs")
FROZEN_FAMILY_ALLOCATIONS = {
    "risk": (
        ("admission_entry", 0, 8),
        ("order_prepare_submit_fill_terminal", 8, 24),
        ("position_exposure", 24, 32),
        ("exit_cancel_replacement", 32, 48),
        ("settlement_redemption", 48, 56),
        ("reconciliation_dependency", 56, 62),
        ("terminal_integrity", 62, 64),
    ),
    "market": (
        ("discovery_identity", 0, 32),
        ("lifecycle_rollover", 32, 80),
        ("subscription_book", 80, 144),
        ("strategy_input_pricing_blocker", 144, 208),
        ("dependency_health", 208, 240),
        ("terminal_closed_window_skip", 240, 256),
    ),
    "system": (
        ("startup_recovery", 0, 16),
        ("storage_archive", 16, 32),
        ("authentication_network_provider", 32, 48),
        ("capacity_host", 48, 60),
        ("integrity_operator", 60, 64),
    ),
}
FROZEN_PRODUCER_CENSUS_ROOTS = ("src",)
FROZEN_READER_CENSUS_ROOTS = (
    "src/bolt_v3_decision_evidence.rs",
    "src/shadow_pnl.rs",
    "scripts/migrate_bolt_v3_decision_evidence_to_v15.py",
)


@dataclass(frozen=True)
class Allocation:
    name: str
    id_start: int
    id_end_exclusive: int


@dataclass(frozen=True)
class Family:
    name: str
    capacity: int
    allocations: tuple[Allocation, ...]


@dataclass(frozen=True)
class State:
    rust_variant: str
    owner: str
    producer_kind: str
    semantic_state: str
    allocation: str
    id: int


@dataclass(frozen=True)
class Reader:
    name: str
    path: str
    symbol: str
    record_kinds: tuple[str, ...]
    recovery_role: bool


@dataclass(frozen=True)
class Producer:
    name: str
    method: str
    record_kind: str
    family: str
    state_id: int
    allocation: str
    classification: str
    handler_reachability: tuple[str, ...]
    call_sites: tuple[str, ...]
    named_readers: tuple[str, ...]
    repeat_semantics: str
    dedupe_key_evidence: str
    recovery_bearing: bool
    suppression: str
    owner_decision_required: bool


@dataclass(frozen=True)
class Registry:
    canonical_state_family: str
    families: tuple[Family, ...]
    states: tuple[State, ...]
    readers: tuple[Reader, ...]
    producers: tuple[Producer, ...]
    producer_census_roots: tuple[str, ...]
    producer_census_exclusions: tuple[str, ...]
    reader_census_roots: tuple[str, ...]
    non_evidence_per_tick_appenders: tuple[str, ...]

    @property
    def family_name(self) -> str:
        return self.canonical_state_family

    @property
    def canonical_family(self) -> Family:
        return next(family for family in self.families if family.name == self.canonical_state_family)

    @property
    def family_capacity(self) -> int:
        return self.canonical_family.capacity

    @property
    def allocations(self) -> tuple[Allocation, ...]:
        return self.canonical_family.allocations


def _strict_keys(value: object, expected: set[str], context: str) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != expected:
        raise ValueError(f"{context} must contain exactly {sorted(expected)}")
    return value


def load_registry(path: pathlib.Path) -> Registry:
    document = _strict_keys(
        tomllib.loads(path.read_text(encoding="utf-8")),
        {
            "schema_version",
            "canonical_state_family",
            "allowed_classifications",
            "allowed_suppression",
            "producer_census_roots",
            "producer_census_exclusions",
            "reader_census_roots",
            "non_evidence_per_tick_appenders",
            "family",
            "state",
            "reader",
            "producer",
        },
        "registry",
    )
    if document["schema_version"] != 2:
        raise ValueError("registry schema_version must be 2")

    raw_families = document["family"]
    if not isinstance(raw_families, list) or not raw_families:
        raise ValueError("registry requires family rows")
    families: list[Family] = []
    for family_index, value in enumerate(raw_families):
        raw_family = _strict_keys(value, {"name", "capacity", "allocations"}, f"family[{family_index}]")
        name = raw_family["name"]
        capacity = raw_family["capacity"]
        if not isinstance(name, str) or not re.fullmatch(r"[a-z][a-z0-9_]*", name):
            raise ValueError(f"family[{family_index}].name must be snake_case")
        if type(capacity) is not int or not 1 <= capacity <= 65_536:
            raise ValueError(f"family[{family_index}].capacity must be an integer in 1..=65536")
        raw_allocations = raw_family["allocations"]
        if not isinstance(raw_allocations, list) or not raw_allocations:
            raise ValueError(f"family[{family_index}] requires allocation rows")
        allocations: list[Allocation] = []
        cursor = 0
        for allocation_index, allocation_value in enumerate(raw_allocations):
            raw_allocation = _strict_keys(
                allocation_value,
                {"name", "start", "end"},
                f"family[{family_index}].allocations[{allocation_index}]",
            )
            row = Allocation(
                name=raw_allocation["name"],
                id_start=raw_allocation["start"],
                id_end_exclusive=raw_allocation["end"],
            )
            if not isinstance(row.name, str) or not re.fullmatch(r"[a-z][a-z0-9_]*", row.name):
                raise ValueError(f"family[{family_index}] allocation name must be snake_case")
            if type(row.id_start) is not int or type(row.id_end_exclusive) is not int:
                raise ValueError(f"family[{family_index}] allocation bounds must be integers")
            if row.id_start != cursor or row.id_end_exclusive <= row.id_start:
                raise ValueError(f"family[{family_index}] allocations must be ordered, contiguous, and non-empty")
            cursor = row.id_end_exclusive
            allocations.append(row)
        if cursor != capacity:
            raise ValueError(f"family[{family_index}] allocations must cover the complete family capacity")
        allocation_names = [row.name for row in allocations]
        if len(set(allocation_names)) != len(allocation_names):
            raise ValueError(f"family[{family_index}] allocation names must be unique")
        families.append(Family(name, capacity, tuple(allocations)))

    if tuple(family.name for family in families) != ("risk", "market", "system"):
        raise ValueError("families must be the frozen ordered set risk, market, system")
    if tuple(family.capacity for family in families) != (64, 256, 64):
        raise ValueError("family capacities must remain risk=64, market=256, system=64")
    for family in families:
        actual_allocations = tuple(
            (allocation.name, allocation.id_start, allocation.id_end_exclusive)
            for allocation in family.allocations
        )
        if actual_allocations != FROZEN_FAMILY_ALLOCATIONS[family.name]:
            raise ValueError(f"family {family.name} allocations must remain frozen")
    canonical_state_family = document["canonical_state_family"]
    if canonical_state_family not in {family.name for family in families}:
        raise ValueError("canonical_state_family must reference a registered family")
    canonical_family = next(family for family in families if family.name == canonical_state_family)
    allocation_names = [row.name for row in canonical_family.allocations]

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
        allocation = canonical_family.allocations[allocation_names.index(row.allocation)]
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

    raw_readers = document["reader"]
    if not isinstance(raw_readers, list) or not raw_readers:
        raise ValueError("registry requires reader rows")
    readers: list[Reader] = []
    reader_keys = {"name", "path", "symbol", "record_kinds", "recovery_role"}
    for index, value in enumerate(raw_readers):
        raw_reader = _strict_keys(value, reader_keys, f"reader[{index}]")
        name = raw_reader["name"]
        reader_path = raw_reader["path"]
        symbol = raw_reader["symbol"]
        record_kinds = raw_reader["record_kinds"]
        recovery_role = raw_reader["recovery_role"]
        if not all(isinstance(item, str) and item for item in (name, reader_path, symbol)):
            raise ValueError(f"reader[{index}] names and path must be non-empty strings")
        if not isinstance(record_kinds, list) or not record_kinds or not all(
            isinstance(kind, str) and re.fullmatch(r"[a-z][a-z0-9_]*|all", kind)
            for kind in record_kinds
        ):
            raise ValueError(f"reader[{index}].record_kinds must be a non-empty kind list")
        if type(recovery_role) is not bool:
            raise ValueError(f"reader[{index}].recovery_role must be boolean")
        readers.append(Reader(name, reader_path, symbol, tuple(record_kinds), recovery_role))
    if len({reader.name for reader in readers}) != len(readers):
        raise ValueError("reader names must be unique")
    if len({(reader.path, reader.symbol) for reader in readers}) != len(readers):
        raise ValueError("reader path/symbol pairs must be unique")

    allowed_classifications = document["allowed_classifications"]
    allowed_suppression = document["allowed_suppression"]
    if allowed_classifications != ["event-keyed", "state-observation", "already-deduped", "no-named-reader"]:
        raise ValueError("allowed_classifications must remain the frozen closed set")
    if allowed_suppression != ["unsuppressed", "finite-monotone-mask"]:
        raise ValueError("allowed_suppression must remain the frozen closed set")
    raw_producers = document["producer"]
    if not isinstance(raw_producers, list) or not raw_producers:
        raise ValueError("registry requires producer rows")
    producers: list[Producer] = []
    producer_keys = {
        "name", "method", "record_kind", "family", "state_id", "allocation",
        "classification", "handler_reachability", "call_sites", "named_readers",
        "repeat_semantics", "dedupe_key_evidence", "recovery_bearing", "suppression",
        "owner_decision_required",
    }
    reader_names = {reader.name for reader in readers}
    families_by_name = {family.name: family for family in families}
    for index, value in enumerate(raw_producers):
        raw_producer = _strict_keys(value, producer_keys, f"producer[{index}]")
        scalar_names = ("name", "method", "record_kind", "family", "allocation", "classification", "repeat_semantics", "dedupe_key_evidence", "suppression")
        if not all(isinstance(raw_producer[key], str) and raw_producer[key] for key in scalar_names):
            raise ValueError(f"producer[{index}] string fields must be non-empty")
        if not re.fullmatch(r"record_[a-z0-9_]+", raw_producer["method"]):
            raise ValueError(f"producer[{index}].method must be a record_* method")
        family = families_by_name.get(raw_producer["family"])
        if family is None:
            raise ValueError(f"producer[{index}] references an unknown family")
        allocations_by_name = {allocation.name: allocation for allocation in family.allocations}
        allocation = allocations_by_name.get(raw_producer["allocation"])
        if allocation is None:
            raise ValueError(f"producer[{index}] references an unknown allocation")
        state_id = raw_producer["state_id"]
        if type(state_id) is not int or not allocation.id_start <= state_id < allocation.id_end_exclusive:
            raise ValueError(f"producer[{index}].state_id is outside its allocation")
        classification = raw_producer["classification"]
        suppression = raw_producer["suppression"]
        if classification not in allowed_classifications:
            raise ValueError(f"producer[{index}] has unknown classification")
        if suppression not in allowed_suppression:
            raise ValueError(f"producer[{index}] has unknown suppression")
        list_fields: dict[str, tuple[str, ...]] = {}
        for key in ("handler_reachability", "call_sites", "named_readers"):
            raw_list = raw_producer[key]
            if not isinstance(raw_list, list) or not all(isinstance(item, str) and item for item in raw_list):
                raise ValueError(f"producer[{index}].{key} must be a string list")
            if len(set(raw_list)) != len(raw_list):
                raise ValueError(f"producer[{index}].{key} must not contain duplicates")
            list_fields[key] = tuple(raw_list)
        if not list_fields["handler_reachability"]:
            raise ValueError(f"producer[{index}] requires handler reachability")
        allowed_reachability = {"startup", "quote", "book", "timer", "index-price", "unreachable"}
        unknown_reachability = set(list_fields["handler_reachability"]) - allowed_reachability
        if unknown_reachability:
            raise ValueError(
                f"producer[{index}] has unknown handler reachability {sorted(unknown_reachability)}"
            )
        if "unreachable" in list_fields["handler_reachability"] and list_fields[
            "handler_reachability"
        ] != ("unreachable",):
            raise ValueError(f"producer[{index}] unreachable cannot be combined with live handlers")
        unreachable = list_fields["handler_reachability"] == ("unreachable",)
        if bool(list_fields["call_sites"]) == unreachable:
            raise ValueError(f"producer[{index}] call sites must be empty exactly when unreachable")
        unknown_readers = set(list_fields["named_readers"]) - reader_names
        if unknown_readers:
            raise ValueError(f"producer[{index}] references unknown readers {sorted(unknown_readers)}")
        recovery_bearing = raw_producer["recovery_bearing"]
        owner_decision_required = raw_producer["owner_decision_required"]
        if type(recovery_bearing) is not bool or type(owner_decision_required) is not bool:
            raise ValueError(f"producer[{index}] boolean fields must be boolean")
        if recovery_bearing and suppression != "unsuppressed":
            raise ValueError(f"producer[{index}] recovery-bearing updates must remain unsuppressed")
        if suppression == "finite-monotone-mask" and recovery_bearing:
            raise ValueError(f"producer[{index}] finite suppression cannot carry recovery")
        if classification == "no-named-reader" and not owner_decision_required:
            raise ValueError(f"producer[{index}] without a domain reader requires an owner decision")
        producers.append(Producer(
            name=raw_producer["name"], method=raw_producer["method"],
            record_kind=raw_producer["record_kind"], family=raw_producer["family"],
            state_id=state_id, allocation=raw_producer["allocation"],
            classification=classification,
            handler_reachability=list_fields["handler_reachability"],
            call_sites=list_fields["call_sites"], named_readers=list_fields["named_readers"],
            repeat_semantics=raw_producer["repeat_semantics"],
            dedupe_key_evidence=raw_producer["dedupe_key_evidence"],
            recovery_bearing=recovery_bearing, suppression=suppression,
            owner_decision_required=owner_decision_required,
        ))
    if len({producer.name for producer in producers}) != len(producers):
        raise ValueError("producer names must be unique")
    producer_ids = [(producer.family, producer.state_id) for producer in producers]
    if len(set(producer_ids)) != len(producer_ids):
        raise ValueError("producer family/state_id pairs must be unique")
    all_call_sites = [call_site for producer in producers for call_site in producer.call_sites]
    if len(set(all_call_sites)) != len(all_call_sites):
        raise ValueError("producer call sites must be classified exactly once")
    producer_record_kinds = {producer.record_kind for producer in producers}
    readers_by_name = {reader.name: reader for reader in readers}
    for reader in readers:
        unknown_kinds = set(reader.record_kinds) - producer_record_kinds - {"all"}
        if unknown_kinds:
            raise ValueError(f"reader {reader.name} references unknown record kinds {sorted(unknown_kinds)}")
    for producer in producers:
        for reader_name in producer.named_readers:
            reader = readers_by_name[reader_name]
            if "all" not in reader.record_kinds and producer.record_kind not in reader.record_kinds:
                raise ValueError(
                    f"producer {producer.name} claims reader {reader_name}, but that reader does not consume {producer.record_kind}"
                )

    def string_tuple(key: str) -> tuple[str, ...]:
        value = document[key]
        if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
            raise ValueError(f"{key} must be a string list")
        return tuple(value)

    producer_census_roots = string_tuple("producer_census_roots")
    reader_census_roots = string_tuple("reader_census_roots")
    if producer_census_roots != FROZEN_PRODUCER_CENSUS_ROOTS:
        raise ValueError("producer_census_roots must remain the complete src tree")
    if reader_census_roots != FROZEN_READER_CENSUS_ROOTS:
        raise ValueError("reader_census_roots must remain the frozen reader authority set")

    return Registry(
        canonical_state_family=canonical_state_family,
        families=tuple(families),
        states=tuple(states),
        readers=tuple(readers),
        producers=tuple(producers),
        producer_census_roots=producer_census_roots,
        producer_census_exclusions=string_tuple("producer_census_exclusions"),
        reader_census_roots=reader_census_roots,
        non_evidence_per_tick_appenders=string_tuple("non_evidence_per_tick_appenders"),
    )


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


@dataclass(frozen=True)
class RustFunction:
    name: str
    start: int
    end: int


def _matching_brace(source: str, open_index: int) -> int:
    depth = 0
    for index in range(open_index, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return index + 1
    raise ValueError("unclosed Rust brace while scanning evidence census")


def _production_rust_source(path: pathlib.Path) -> str:
    source = strip_rust_comments_and_literals(path.read_text(encoding="utf-8"))
    mutable = list(source)
    cfg_test = re.compile(
        r"#\s*\[\s*cfg\s*\([^\]]*\btest\b[^\]]*\)\s*\]"
        r"(?=\s*(?:(?:pub(?:\([^)]*\))?|async|unsafe|const)\s+)*"
        r"(?:mod|fn|impl|use|struct|enum|static|type|trait)\b)"
    )
    for match in tuple(cfg_test.finditer(source)):
        open_index = source.find("{", match.end())
        semicolon_index = source.find(";", match.end())
        if semicolon_index != -1 and (open_index == -1 or semicolon_index < open_index):
            end = semicolon_index + 1
        elif open_index != -1:
            end = _matching_brace(source, open_index)
        else:
            end = len(source)
        for index in range(match.start(), end):
            if mutable[index] != "\n":
                mutable[index] = " "
    return "".join(mutable)


def _rust_functions(source: str) -> tuple[RustFunction, ...]:
    functions: list[RustFunction] = []
    pattern = re.compile(
        r"^[ \t]*(?:(?:pub(?:\([^)]*\))?|async|unsafe|const|extern\s+\"[^\"]+\")\s+)*"
        r"fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^{};]*>)?\s*\(",
        re.MULTILINE,
    )
    for match in pattern.finditer(source):
        open_index = source.find("{", match.end())
        semicolon_index = source.find(";", match.end())
        if open_index == -1 or (semicolon_index != -1 and semicolon_index < open_index):
            continue
        functions.append(RustFunction(match.group(1), match.start(), _matching_brace(source, open_index)))
    return tuple(functions)


def _trait_record_methods(source: str) -> set[str]:
    trait_start = source.find("pub trait BoltV3DecisionEvidenceWriter")
    if trait_start == -1:
        raise ValueError("missing BoltV3DecisionEvidenceWriter trait")
    open_index = source.find("{", trait_start)
    trait_body = source[open_index:_matching_brace(source, open_index)]
    return set(re.findall(r"\bfn\s+(record_[a-z0-9_]+)\s*\(", trait_body))


def _producer_call_sites(root: pathlib.Path, registry: Registry) -> set[str]:
    methods = {producer.method for producer in registry.producers}
    method_pattern = re.compile(
        r"(?:\.|::)\s*(" + "|".join(sorted(map(re.escape, methods))) + r")\s*\("
    )
    call_sites: set[str] = set()
    for configured_root in registry.producer_census_roots:
        source_root = root / configured_root
        paths = source_root.rglob("*.rs") if source_root.is_dir() else (source_root,)
        for path in paths:
            relative = path.relative_to(root)
            if "tests" in relative.parts:
                continue
            source = _production_rust_source(path)
            functions = _rust_functions(source)
            ordinals: dict[tuple[str, str], int] = {}
            for call in method_pattern.finditer(source):
                enclosing = [function for function in functions if function.start <= call.start() < function.end]
                if not enclosing:
                    raise ValueError(f"evidence call outside a Rust function: {relative}:{call.start()}")
                function = min(enclosing, key=lambda item: item.end - item.start)
                method = call.group(1)
                key = (function.name, method)
                ordinals[key] = ordinals.get(key, 0) + 1
                call_sites.add(f"{relative.as_posix()}::{function.name}::{method}::{ordinals[key]}")
    return call_sites


def _reader_census(root: pathlib.Path, registry: Registry) -> set[tuple[str, str]]:
    readers: set[tuple[str, str]] = set()
    for configured_root in registry.reader_census_roots:
        path = root / configured_root
        if path.suffix == ".rs":
            source = _production_rust_source(path)
            for function in _rust_functions(source):
                if function.name.startswith("read_"):
                    readers.add((configured_root, function.name))
        elif path.suffix == ".py":
            module = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
            for node in module.body:
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name in {
                    "migrate_file_bytes",
                    "plan_migrations",
                }:
                    readers.add((configured_root, node.name))
        else:
            raise ValueError(f"unsupported reader census root {configured_root}")
    return readers


def _non_evidence_per_tick_appenders(root: pathlib.Path) -> set[str]:
    appenders: set[str] = set()
    patterns = re.compile(
        r"\b(?:OpenOptions|BufWriter)\b|\bFile::(?:create|open)\s*\(|\b(?:std::)?fs::write\s*\(|\.write_all\s*\(|\.append\s*\(\s*true\s*\)"
    )
    sweep_paths = [root / "src/strategies", root / "src/bolt_v3_live_node.rs", root / "src/bolt_v3_live_node"]
    for sweep_root in sweep_paths:
        paths = sweep_root.rglob("*.rs") if sweep_root.is_dir() else (sweep_root,)
        for path in paths:
            relative = path.relative_to(root)
            if "tests" in relative.parts:
                continue
            source = _production_rust_source(path)
            for match in patterns.finditer(source):
                line = source.count("\n", 0, match.start()) + 1
                appenders.add(f"{relative.as_posix()}:{line}")
    return appenders


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
        decision_evidence_source = _production_rust_source(root / DECISION_EVIDENCE_PATH)
        trait_methods = _trait_record_methods(decision_evidence_source)
        registered_methods = {producer.method for producer in registry.producers}
        if trait_methods != registered_methods:
            errors.append(
                "producer methods must exactly cover BoltV3DecisionEvidenceWriter: "
                f"missing={sorted(trait_methods - registered_methods)} "
                f"unknown={sorted(registered_methods - trait_methods)}"
            )
        source_record_kinds = set(
            re.findall(
                r"\bconst\s+BOLT_V3_[A-Z0-9_]+_RECORD_KIND\s*:\s*&str\s*=\s*\"([a-z][a-z0-9_]*)\"",
                (root / DECISION_EVIDENCE_PATH).read_text(encoding="utf-8"),
            )
        )
        registered_record_kinds = {producer.record_kind for producer in registry.producers}
        if source_record_kinds != registered_record_kinds:
            errors.append(
                "producer record kinds must exactly cover source constants: "
                f"missing={sorted(source_record_kinds - registered_record_kinds)} "
                f"unknown={sorted(registered_record_kinds - source_record_kinds)}"
            )

        discovered_call_sites = _producer_call_sites(root, registry)
        exclusions = set(registry.producer_census_exclusions)
        missing_exclusions = exclusions - discovered_call_sites
        if missing_exclusions:
            errors.append(f"producer census exclusions are stale: {sorted(missing_exclusions)}")
        discovered_call_sites -= exclusions
        registered_call_sites = {
            call_site for producer in registry.producers for call_site in producer.call_sites
        }
        if discovered_call_sites != registered_call_sites:
            errors.append(
                "producer call sites must be classified exactly once: "
                f"missing={sorted(discovered_call_sites - registered_call_sites)} "
                f"stale={sorted(registered_call_sites - discovered_call_sites)}"
            )

        discovered_readers = _reader_census(root, registry)
        registered_readers = {(reader.path, reader.symbol) for reader in registry.readers}
        if discovered_readers != registered_readers:
            errors.append(
                "reader census must exactly match source: "
                f"missing={sorted(discovered_readers - registered_readers)} "
                f"stale={sorted(registered_readers - discovered_readers)}"
            )

        discovered_appenders = _non_evidence_per_tick_appenders(root)
        registered_appenders = set(registry.non_evidence_per_tick_appenders)
        if discovered_appenders != registered_appenders:
            errors.append(
                "non-evidence per-tick appender sweep must exactly match the registry: "
                f"missing={sorted(discovered_appenders - registered_appenders)} "
                f"stale={sorted(registered_appenders - discovered_appenders)}"
            )
    except (OSError, UnicodeDecodeError, SyntaxError, ValueError) as error:
        errors.append(str(error))

    producer: str | None = None
    try:
        producer = strip_rust_comments_and_literals(
            (root / PRODUCER_PATH).read_text(encoding="utf-8")
        )
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

    try:
        entry_decision = strip_rust_comments_and_literals(
            (root / ENTRY_DECISION_PATH).read_text(encoding="utf-8")
        )
    except OSError as error:
        errors.append(str(error))
    else:
        if producer is None:
            return errors
        defined_reasons = set(
            re.findall(r"\bconst\s+(ENTRY_BLOCK_REASON_[A-Z0-9_]+)\s*:", producer)
        )
        mapped_reasons = set(
            re.findall(r"^\s*(ENTRY_BLOCK_REASON_[A-Z0-9_]+)\s*=>", entry_decision, re.MULTILINE)
        )
        if defined_reasons != mapped_reasons:
            errors.append(
                "entry-skip reason mappings must exactly cover runtime entry-block reasons: "
                f"missing={sorted(defined_reasons - mapped_reasons)} "
                f"unknown={sorted(mapped_reasons - defined_reasons)}"
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
