#!/usr/bin/env python3
"""Fail-closed verifier for the #1354 evidence producer/reader registry."""

from __future__ import annotations

import argparse
import ast
import dataclasses
import pathlib
import re
import sys
import tomllib
from typing import Any

from rust_source_scanner import (
    RUST_OPEN_TO_CLOSE,
    RustToken,
    rust_tokens_and_delimiter_pairs,
    strip_rust_comments_and_literals,
)


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
    "producer_census_roots",
    "producer_census_exclusions",
    "reader_census_roots",
    "family",
    "reader",
    "producer",
}
FAMILY_KEYS = {"name", "capacity", "allocations"}
ALLOCATION_KEYS = {"name", "start", "end"}
READER_KEYS = {"name", "path", "symbol", "record_kinds", "recovery_role"}
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
    "call_sites",
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
EXPECTED_SUPPRESSION = {
    "unsuppressed",
    "current-state-bounded",
    "legacy-monotone-mask",
    "finite-episode",
}
EXPECTED_HANDLER_CLASSES = {"quote", "book", "timer", "index-price", "startup"}
EXPECTED_NON_EVIDENCE_RECORD_NAME_COLLISIONS = {
    "src/bolt_v3_capital_admission_runtime_feed.rs::record_venue_truth_settlement::record_settlement::1",
    "src/bolt_v3_venue_truth.rs::record_settlement::record_settlement::1",
}
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


@dataclasses.dataclass(frozen=True)
class RustFunction:
    symbol: str
    body: tuple[RustToken, ...]


def rust_function_definitions(path: pathlib.Path) -> list[RustFunction]:
    masked = strip_rust_comments_and_literals(path.read_text(encoding="utf-8"))
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        raise RegistryError(f"Rust structural parse failed for {path.relative_to(ROOT)}")
    tokens, pairs = tokenized
    test_items: list[tuple[int, int, int]] = []
    for index, token in enumerate(tokens[:-1]):
        if token.value != "#" or tokens[index + 1].value != "[":
            continue
        attribute_end = pairs.get(index + 1)
        if attribute_end is None or "test" not in {
            item.value.removeprefix("r#")
            for item in tokens[index + 2 : attribute_end]
        }:
            continue
        cursor = attribute_end + 1
        while cursor < len(tokens) and tokens[cursor].value not in {"{", ";"}:
            cursor += 1
        if cursor < len(tokens) and tokens[cursor].value == "{":
            test_items.append((attribute_end, cursor, pairs[cursor]))
    functions: list[RustFunction] = []
    for index, token in enumerate(tokens):
        if token.value != "fn" or index + 1 >= len(tokens):
            continue
        if any(
            opening < index < closing or attribute_end < index < opening
            for attribute_end, opening, closing in test_items
        ):
            continue
        symbol = tokens[index + 1].value.removeprefix("r#")
        cursor = index + 2
        while cursor < len(tokens) and tokens[cursor].value not in {"(", "{", ";"}:
            if tokens[cursor].value in RUST_OPEN_TO_CLOSE:
                cursor = pairs[cursor] + 1
            else:
                cursor += 1
        if cursor >= len(tokens) or tokens[cursor].value != "(":
            continue
        cursor = pairs[cursor] + 1
        while cursor < len(tokens) and tokens[cursor].value not in {"{", ";"}:
            if tokens[cursor].value in {"(", "["}:
                cursor = pairs[cursor] + 1
            else:
                cursor += 1
        if cursor < len(tokens) and tokens[cursor].value == "{":
            closing = pairs[cursor]
            functions.append(RustFunction(symbol, tuple(tokens[cursor + 1 : closing])))
    return functions


def source_trait_methods(source: str) -> set[str]:
    masked = strip_rust_comments_and_literals(source)
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        raise RegistryError("source: decision-evidence trait has invalid Rust structure")
    tokens, pairs = tokenized
    for index in range(len(tokens) - 2):
        if [token.value for token in tokens[index : index + 2]] != [
            "trait",
            "BoltV3DecisionEvidenceWriter",
        ]:
            continue
        opening = next(
            (cursor for cursor in range(index + 2, len(tokens)) if tokens[cursor].value == "{"),
            None,
        )
        if opening is None:
            break
        closing = pairs[opening]
        return {
            tokens[cursor + 1].value.removeprefix("r#")
            for cursor in range(opening + 1, closing - 1)
            if tokens[cursor].value == "fn"
            and tokens[cursor + 1].value.removeprefix("r#").startswith("record_")
        }
    raise RegistryError("source: cannot locate BoltV3DecisionEvidenceWriter trait")


def rust_record_calls(path: pathlib.Path, methods: set[str]) -> set[str]:
    calls: set[str] = set()
    violations: list[str] = []
    for function in rust_function_definitions(path):
        counts: dict[str, int] = {}
        body = function.body
        for index, token in enumerate(body):
            method = token.value.removeprefix("r#")
            if method not in methods:
                continue
            predecessor = body[index - 1].value if index else None
            successor = body[index + 1].value if index + 1 < len(body) else None
            canonical = predecessor in {".", "::"} and successor == "("
            if not canonical:
                violations.append(
                    f"{path.relative_to(ROOT)}::{function.symbol}::{method}"
                )
                continue
            counts[method] = counts.get(method, 0) + 1
            calls.add(
                f"{path.relative_to(ROOT)}::{function.symbol}::{method}::{counts[method]}"
            )
    masked = strip_rust_comments_and_literals(path.read_text(encoding="utf-8"))
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        raise RegistryError(f"producer structural parse failed for {path.relative_to(ROOT)}")
    tokens, pairs = tokenized
    test_items: list[tuple[int, int]] = []
    for index, token in enumerate(tokens[:-1]):
        if token.value != "#" or tokens[index + 1].value != "[":
            continue
        attribute_end = pairs.get(index + 1)
        if attribute_end is None or "test" not in {
            item.value.removeprefix("r#")
            for item in tokens[index + 2 : attribute_end]
        }:
            continue
        cursor = attribute_end + 1
        while cursor < len(tokens) and tokens[cursor].value not in {"{", ";"}:
            cursor += 1
        if cursor < len(tokens) and tokens[cursor].value == "{":
            test_items.append((attribute_end, pairs[cursor]))
    for index, token in enumerate(tokens):
        if any(start < index < end for start, end in test_items):
            continue
        method = token.value.removeprefix("r#")
        if method not in methods:
            continue
        predecessor = tokens[index - 1].value if index else None
        successor = tokens[index + 1].value if index + 1 < len(tokens) else None
        if predecessor == "fn" or (predecessor in {".", "::"} and successor == "("):
            continue
        violations.append(f"{path.relative_to(ROOT)}::<module>::{method}")
    if violations:
        raise RegistryError(
            f"producer structural authority forbids alias/wrapper/macro dispatch: {sorted(violations)}"
        )
    return calls


def source_structural_calls(data: dict[str, Any], methods: set[str]) -> set[str]:
    calls: set[str] = set()
    for relative_root in data["producer_census_roots"]:
        root = ROOT / relative_root
        paths = [root] if root.is_file() else sorted(root.rglob("*.rs"))
        for path in paths:
            if "tests" in path.relative_to(ROOT).parts:
                continue
            calls.update(rust_record_calls(path, methods))
    exclusions = set(data["producer_census_exclusions"])
    missing_exclusions = exclusions - calls
    if missing_exclusions:
        raise RegistryError(
            f"producer census exclusions are stale: {sorted(missing_exclusions)}"
        )
    return calls - exclusions


RUST_EVIDENCE_AUTHORITY = "src/bolt_v3_decision_evidence.rs"
PYTHON_EVIDENCE_AUTHORITY = "scripts/migrate_bolt_v3_decision_evidence_to_v15.py"
SEALED_RUST_READER_SYMBOLS = {
    "read_decision_evidence_jsonl_lines",
    "read_jsonl_lines",
    "read_kind_evidence",
}
RUST_RAW_IO_METHODS = {
    "open",
    "options",
    "read",
    "read_to_end",
    "read_to_string",
    "write",
    "write_all",
}
FROZEN_NON_EVIDENCE_RAW_IO = {
    "src/bolt_v3_atomic_io.rs::append_private_file",
    "src/bolt_v3_atomic_io.rs::sync_parent_dir",
    "src/bolt_v3_atomic_io.rs::write_private_new_file",
    "src/bolt_v3_atomic_io.rs::write_synced_temp_file",
    "src/bolt_v3_basket_store.rs::load_recovery_state",
    "src/bolt_v3_capital_admission_state.rs::read_file_bounded",
    "src/bolt_v3_config.rs::load_bolt_v3_config",
    "src/bolt_v3_deploy_target.rs::load_deploy_target",
    "src/bolt_v3_iv/capability.rs::load_capability_ledger_fixture",
    "src/bolt_v3_iv/capability.rs::scan_candidates",
    "src/bolt_v3_iv/capability.rs::scan_seed_families",
    "src/bolt_v3_iv/query.rs::read_state",
    "src/bolt_v3_iv/query.rs::write_state",
    "src/bolt_v3_iv/runtime.rs::read_inner",
    "src/bolt_v3_iv/runtime.rs::write_inner",
    "src/bolt_v3_kill_switch_store.rs::load_loss_governor_manual_recoveries",
    "src/bolt_v3_kill_switch_store.rs::load_recovery_record",
    "src/bolt_v3_kill_switch_store.rs::loss_governor_manual_recovery_audit_appendable_line_count",
    "src/bolt_v3_loss_governor_manual_recovery_ops.rs::write_clock_refusal",
    "src/bolt_v3_operator_artifacts.rs::read_file_bounded",
    "src/bolt_v3_operator_artifacts.rs::write_json_artifact_create_new",
    "src/bolt_v3_prod_profile.rs::read_config_text",
    "src/bolt_v3_providers/hyperliquid_artifacts.rs::open_hyperliquid_live_submit_approval_file_for_spend",
    "src/bolt_v3_providers/hyperliquid_artifacts.rs::persist_consumed_hyperliquid_live_submit_approval_artifact",
    "src/bolt_v3_providers/hyperliquid_artifacts.rs::read_open_file_bounded",
    "src/bolt_v3_providers/reference_boundary_capture.rs::capture_reference_boundary_fixture",
    "src/bolt_v3_realized_volatility.rs::config_fingerprint",
    "src/bolt_v3_realized_volatility.rs::write_estimator_fingerprint",
    "src/bounded_config_read.rs::read_to_string",
    "src/execution_state.rs::read_jsonl_rows",
    "src/execution_state.rs::write_record_batch",
    "src/lake_batch.rs::acquire",
    "src/lake_batch.rs::open_feather_reader",
    "src/lake_batch.rs::write_atomic_file",
    "src/main.rs::emit_ops_launch_stage_log",
    "src/main.rs::print_reference_current_price_health_report",
    "src/main.rs::run_reference_current_price_health_subprocess",
    "src/main.rs::verify_catalog_write_probe",
    "src/nt_runtime_capture.rs::write_capture_message",
    "src/raw_types.rs::append",
    "src/raw_types.rs::append_jsonl",
    "src/raw_types.rs::ensure_path",
    "src/shadow_pnl.rs::write_shadow_pnl_csv",
    "src/shadow_pnl.rs::write_shadow_pnl_csv_header",
    "src/source_canonicalization.rs::read_file_bounded",
    "src/venue_contract.rs::load_and_validate",
}


def rust_function_calls(function: RustFunction) -> set[str]:
    return {
        token.value.removeprefix("r#")
        for index, token in enumerate(function.body[:-1])
        if function.body[index + 1].value == "("
    }


def rust_function_has_raw_io(function: RustFunction) -> bool:
    values = [token.value.removeprefix("r#") for token in function.body]
    for index, value in enumerate(values):
        if value in {"write", "writeln"} and index + 1 < len(values) and values[index + 1] == "!":
            return True
        if value in {"to_writer", "to_writer_pretty"} and index and values[index - 1] == "::":
            return True
        if value in RUST_RAW_IO_METHODS and index + 1 < len(values) and values[index + 1] == "(":
            predecessor = values[index - 1] if index else None
            if predecessor in {".", "::"}:
                return True
    return False


def python_function_definitions(path: pathlib.Path) -> dict[str, ast.AST]:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    return {
        node.name: node
        for node in ast.walk(tree)
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    }


def python_function_has_raw_read(node: ast.AST) -> bool:
    for child in ast.walk(node):
        if not isinstance(child, ast.Call):
            continue
        if isinstance(child.func, ast.Attribute) and child.func.attr in {
            "read",
            "read_bytes",
            "read_text",
            "readline",
            "readlines",
        }:
            return True
        if isinstance(child.func, ast.Name) and child.func.id == "open":
            return True
    return False


def validate_frozen_rust_io_authority(data: dict[str, Any]) -> None:
    observed: set[str] = set()
    for relative_root in data["reader_census_roots"]:
        root = ROOT / relative_root
        if root.suffix == ".py" or relative_root == "scripts":
            continue
        paths = [root] if root.is_file() else sorted(root.rglob("*.rs"))
        for path in paths:
            if "tests" in path.relative_to(ROOT).parts:
                continue
            relative = str(path.relative_to(ROOT))
            if relative == RUST_EVIDENCE_AUTHORITY:
                continue
            observed.update(
                f"{relative}::{function.symbol}"
                for function in rust_function_definitions(path)
                if function.symbol != "fmt" and rust_function_has_raw_io(function)
            )
    if observed != FROZEN_NON_EVIDENCE_RAW_IO:
        raise RegistryError(
            "evidence I/O authority whole-tree raw-I/O census mismatch: "
            f"new={sorted(observed - FROZEN_NON_EVIDENCE_RAW_IO)} "
            f"missing={sorted(FROZEN_NON_EVIDENCE_RAW_IO - observed)}"
        )


def validate_structural_reader_authority(
    registered: set[tuple[str, str]], data: dict[str, Any]
) -> None:
    validate_frozen_rust_io_authority(data)
    rust_by_path: dict[str, list[RustFunction]] = {}
    python_by_path: dict[str, dict[str, ast.AST]] = {}
    for path, symbol in registered:
        absolute = ROOT / path
        if not absolute.is_file():
            raise RegistryError(f"reader structural authority missing path {path!r}")
        if absolute.suffix == ".rs":
            functions = rust_by_path.setdefault(path, rust_function_definitions(absolute))
            if symbol not in {function.symbol for function in functions}:
                raise RegistryError(
                    "reader census mismatch: reader structural authority missing Rust symbol "
                    f"{path}::{symbol}"
                )
        elif absolute.suffix == ".py":
            functions = python_by_path.setdefault(path, python_function_definitions(absolute))
            if symbol not in functions:
                raise RegistryError(
                    "reader census mismatch: reader structural authority missing Python symbol "
                    f"{path}::{symbol}"
                )
        else:
            raise RegistryError(f"reader structural authority rejects file type for {path!r}")

    registered_by_path: dict[str, set[str]] = {}
    for path, symbol in registered:
        registered_by_path.setdefault(path, set()).add(symbol)

    for path, registered_symbols in registered_by_path.items():
        if not path.endswith(".rs"):
            continue
        functions = rust_by_path[path]
        by_symbol = {function.symbol: function for function in functions}
        for function in functions:
            calls = rust_function_calls(function)
            if (
                path != RUST_EVIDENCE_AUTHORITY
                and function.symbol not in registered_symbols
                and calls & SEALED_RUST_READER_SYMBOLS
            ):
                raise RegistryError(
                    "reader structural authority rejects unregistered wrapper "
                    f"{path}::{function.symbol}"
                )
        for symbol in registered_symbols:
            if path == RUST_EVIDENCE_AUTHORITY:
                continue
            pending = [symbol]
            visited: set[str] = set()
            while pending:
                current = pending.pop()
                if current in visited:
                    continue
                visited.add(current)
                function = by_symbol[current]
                if rust_function_has_raw_io(function):
                    raise RegistryError(
                        "evidence I/O authority rejects raw Rust I/O outside "
                        f"{RUST_EVIDENCE_AUTHORITY}: {path}::{current}"
                    )
                pending.extend(rust_function_calls(function) & set(by_symbol))

    for relative_root in data["reader_census_roots"]:
        root = ROOT / relative_root
        paths = [root] if root.is_file() else sorted(root.rglob("*.py"))
        for path in paths:
            if (
                not path.is_file()
                or "tests" in path.relative_to(ROOT).parts
                or path.name.startswith("test_")
            ):
                continue
            relative = str(path.relative_to(ROOT))
            source = path.read_text(encoding="utf-8")
            if relative == PYTHON_EVIDENCE_AUTHORITY:
                functions = python_by_path.setdefault(relative, python_function_definitions(path))
                for symbol, node in functions.items():
                    if python_function_has_raw_read(node) and symbol != "plan_migrations":
                        raise RegistryError(
                            "Python evidence I/O authority rejects unregistered reader "
                            f"{relative}::{symbol}"
                        )
                continue
            tree = ast.parse(source, filename=str(path))
            declares_evidence_authority = any(
                isinstance(node, ast.Name) and "DECISION_EVIDENCE" in node.id
                for node in ast.walk(tree)
            )
            if not declares_evidence_authority:
                continue
            functions = {
                node.name: node
                for node in ast.walk(tree)
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
            }
            raw_readers = sorted(
                symbol for symbol, node in functions.items() if python_function_has_raw_read(node)
            )
            if raw_readers:
                raise RegistryError(
                    "Python evidence I/O authority rejects new tool readers: "
                    f"{relative}::{raw_readers}"
                )


def validate_readers(data: dict[str, Any]) -> set[str]:
    rows = data["reader"]
    names: set[str] = set()
    paths: set[tuple[str, str]] = set()
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
        concrete = (row["path"], row["symbol"])
        if concrete in paths:
            raise RegistryError(f"reader[{index}]: duplicate path/symbol {concrete!r}")
        paths.add(concrete)
    validate_structural_reader_authority(paths, data)
    expected_recovery = {
        ("src/bolt_v3_decision_evidence.rs", "read_latest_entry_decision_evidence_chain"),
        ("src/bolt_v3_decision_evidence.rs", "read_submit_reservation_recovery_evidence"),
        (
            "src/bolt_v3_decision_evidence.rs",
            "read_terminal_settlement_keys_for_recovery_scope",
        ),
        ("src/bolt_v3_decision_evidence.rs", "read_settlement_keys_for_recovery_scope"),
        (
            "src/bolt_v3_decision_evidence.rs",
            "read_settlement_booking_error_keys_for_recovery_scope",
        ),
        (
            "src/bolt_v3_decision_evidence.rs",
            "read_settlement_evidence_for_recovery_scope",
        ),
        ("src/bolt_v3_decision_evidence.rs", "read_settlement_evidence_records"),
        ("src/bolt_v3_decision_evidence.rs", "read_kind_evidence"),
    }
    observed_recovery = {
        (row["path"], row["symbol"]) for row in rows if row["recovery_role"]
    }
    if observed_recovery != expected_recovery:
        raise RegistryError("reader census recovery-role classification mismatch")
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
    if len(rows) != 20:
        raise RegistryError(f"producer census must contain 20 structural rows, found {len(rows)}")
    names: set[str] = set()
    family_ids: set[tuple[str, int]] = set()
    methods: dict[str, int] = {}
    call_sites: set[str] = set()
    reader_kinds = {
        row["name"]: set(row["record_kinds"])
        for row in data["reader"]
        if row["name"]
        not in {"read_kind_evidence", "runtime_read_decision_evidence_jsonl_lines"}
    }
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
        if row["classification"] == "no-named-reader" and any(
            not reader.startswith("v15_migrator_") for reader in row["named_readers"]
        ):
            raise RegistryError(
                f"producer {name}: no-named-reader row has a semantic consumer"
            )
        if set(row["named_readers"]) - readers:
            raise RegistryError(f"producer {name}: names an unknown reader")
        expected_readers = {
            reader_name
            for reader_name, kinds in reader_kinds.items()
            if row["record_kind"] in kinds or "all" in kinds
        }
        if set(row["named_readers"]) != expected_readers:
            raise RegistryError(
                f"producer {name}: reader mapping mismatch "
                f"missing={sorted(expected_readers - set(row['named_readers']))} "
                f"unknown={sorted(set(row['named_readers']) - expected_readers)}"
            )
        if not row["handler_reachability"] or set(row["handler_reachability"]) - EXPECTED_HANDLER_CLASSES:
            raise RegistryError(f"producer {name}: invalid handler reachability")
        if not row["call_sites"] or not row["repeat_semantics"] or not row["dedupe_key_evidence"]:
            raise RegistryError(f"producer {name}: incomplete census evidence")
        for call_site in row["call_sites"]:
            if f"::{row['method']}::" not in call_site:
                raise RegistryError(f"producer {name}: callsite method mismatch {call_site!r}")
            if call_site in call_sites:
                raise RegistryError(f"producer {name}: duplicate callsite {call_site!r}")
            call_sites.add(call_site)
        if name == "strategy_input_snapshot_submit" and not all(
            "::submit_admitted_entry_decision::" in call_site
            for call_site in row["call_sites"]
        ):
            raise RegistryError("submit snapshot row must own only its structural submit callsite")
        if name == "strategy_input_snapshot_blocked_rv" and not all(
            "::record_blocked_entry_strategy_input_snapshot_once::" in call_site
            for call_site in row["call_sites"]
        ):
            raise RegistryError("blocked snapshot row must own only its structural blocked callsite")
        if row["suppression"] == "finite-episode":
            raise RegistryError(
                f"producer {name}: finite episode suppression is deferred until typed Gamma binding exists"
            )
        methods[row["method"]] = methods.get(row["method"], 0) + 1

    actual_methods = source_trait_methods(source)
    if len(actual_methods) != 19:
        raise RegistryError(f"producer trait census must contain 19 methods, found {len(actual_methods)}")
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
    actual_calls = source_structural_calls(data, actual_methods)
    if call_sites != actual_calls:
        raise RegistryError(
            "producer structural authority; producer callsite census mismatch: "
            f"missing={sorted(actual_calls - call_sites)} "
            f"unknown={sorted(call_sites - actual_calls)}"
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
    if data["producer_census_roots"] != ["src"]:
        raise RegistryError("registry: producer census must cover the complete src tree")
    if set(data["producer_census_exclusions"]) != EXPECTED_NON_EVIDENCE_RECORD_NAME_COLLISIONS:
        raise RegistryError("registry: producer census exclusions differ from reviewed collisions")
    if data["reader_census_roots"] != ["src", "scripts"]:
        raise RegistryError("registry: reader census must cover src and scripts")


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
        "order_id",
        "client_order",
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
    for type_name in identity_types:
        type_impl = re.search(
            rf"impl {type_name}\s*\{{(?P<body>.*?)\n\}}", source, re.DOTALL
        )
        if type_impl is None or not re.search(
            r"#\[cfg\(test\)\]\s*fn new\s*\(", type_impl.group("body")
        ):
            raise RegistryError(f"identity: {type_name} construction must remain test-only")
        if re.search(r"pub(?:\([^)]*\))?\s+fn new\s*\(", type_impl.group("body")):
            raise RegistryError("identity: production constructor authority is forbidden")
    if not re.search(
        r"pub fn encode_canonical\s*\(&self\)\s*->\s*Vec<u8>", source
    ):
        raise RegistryError("identity: canonical encoder contract is missing")
    for path in ROOT.joinpath("src").rglob("*.rs"):
        if path == ROOT / data["identity_module"]:
            continue
        other = path.read_text(encoding="utf-8")
        alternate_construction = any(
            f"{type_name}::new" in other
            or re.search(rf"{type_name}\s*\{{", other)
            for type_name in identity_types
        )
        if alternate_construction:
            raise RegistryError(
                f"identity: alternate construction found in {path.relative_to(ROOT)}"
            )
        if "encode_canonical" in other:
            raise RegistryError(
                f"identity: production encoder consumption found in {path.relative_to(ROOT)}"
            )


def rust_named_struct_fields(path: pathlib.Path, struct_name: str) -> dict[str, tuple[str, ...]]:
    masked = strip_rust_comments_and_literals(path.read_text(encoding="utf-8"))
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        raise RegistryError(f"guard shape: invalid Rust structure in {path.relative_to(ROOT)}")
    tokens, pairs = tokenized
    for index in range(len(tokens) - 2):
        if tokens[index].value != "struct" or tokens[index + 1].value != struct_name:
            continue
        opening = index + 2
        while opening < len(tokens) and tokens[opening].value != "{":
            opening += 1
        if opening >= len(tokens):
            break
        closing = pairs[opening]
        fields: dict[str, tuple[str, ...]] = {}
        cursor = opening + 1
        while cursor < closing:
            if tokens[cursor].value in {"pub", "(", ")", ","}:
                cursor += 1
                continue
            name = tokens[cursor].value.removeprefix("r#")
            if cursor + 1 >= closing or tokens[cursor + 1].value != ":":
                cursor += 1
                continue
            type_start = cursor + 2
            type_end = type_start
            while type_end < closing and tokens[type_end].value != ",":
                if tokens[type_end].value in RUST_OPEN_TO_CLOSE:
                    type_end = pairs[type_end] + 1
                else:
                    type_end += 1
            fields[name] = tuple(token.value for token in tokens[type_start:type_end])
            cursor = type_end + 1
        return fields
    raise RegistryError(
        f"guard shape: cannot locate {path.relative_to(ROOT)}::{struct_name}"
    )


def validate_private_u16_newtype(
    path: pathlib.Path,
    type_name: str,
    static_name: str,
    domain_cardinality: int,
) -> None:
    source = path.read_text(encoding="utf-8")
    masked = strip_rust_comments_and_literals(source)
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        raise RegistryError(f"guard shape: invalid Rust structure in {path.relative_to(ROOT)}")
    tokens, pairs = tokenized
    for index in range(len(tokens) - 2):
        if tokens[index].value != "struct" or tokens[index + 1].value != type_name:
            continue
        if index and tokens[index - 1].value == "pub":
            raise RegistryError(f"guard shape: {type_name} storage type must remain private")
        opening = index + 2
        if tokens[opening].value != "(":
            raise RegistryError(f"guard shape: {type_name} must be an exact u16 newtype")
        closing = pairs[opening]
        storage = [token.value for token in tokens[opening + 1 : closing]]
        if storage != ["AtomicU16"]:
            raise RegistryError(
                f"guard shape: {type_name} must contain only private AtomicU16 storage, got {storage}"
            )
        expected_static = [
            "static",
            static_name,
            ":",
            type_name,
            "=",
            type_name,
            "::",
            "new",
            "(",
            ")",
            ";",
        ]
        values = [token.value for token in tokens]
        if not any(
            values[cursor : cursor + len(expected_static)] == expected_static
            for cursor in range(len(values) - len(expected_static) + 1)
        ):
            raise RegistryError(
                f"guard shape: {type_name} requires one process-lifetime static {static_name}"
            )
        if values.count(static_name) != 2:
            raise RegistryError(
                f"guard shape: {static_name} may appear only in its declaration and sealed accessor"
            )
        if (
            f"std::mem::size_of::<Legacy" not in source
            or f"std::mem::size_of::<{type_name}>()" not in source
        ):
            raise RegistryError(f"guard shape: {type_name} requires compile-time u16 size proof")
        impl_opening = next(
            (
                cursor + 2
                for cursor in range(len(tokens) - 2)
                if tokens[cursor].value == "impl"
                and tokens[cursor + 1].value == type_name
                and tokens[cursor + 2].value == "{"
            ),
            None,
        )
        if impl_opening is None:
            raise RegistryError(f"guard shape: cannot locate implementation for {type_name}")
        body = [
            token.value
            for token in tokens[impl_opening + 1 : pairs[impl_opening]]
        ]
        impl_text = masked[
            tokens[impl_opening].end : tokens[pairs[impl_opening]].start
        ]
        expected_domain = [
            "const",
            "DOMAIN_CARDINALITY",
            ":",
            "u32",
            "=",
            str(domain_cardinality),
            ";",
        ]
        if not any(
            body[cursor : cursor + len(expected_domain)] == expected_domain
            for cursor in range(len(body) - len(expected_domain) + 1)
        ):
            raise RegistryError(
                f"guard shape: {type_name} must freeze domain cardinality {domain_cardinality}"
            )
        if re.search(r"self\s*\.\s*0\s*=", impl_text):
            raise RegistryError(f"guard shape: {type_name} storage may only change monotonically")
        forbidden_methods = {"clear", "reset", "replace", "evict", "remove"}
        if any(
            value == "fn" and cursor + 1 < len(body) and body[cursor + 1] in forbidden_methods
            for cursor, value in enumerate(body)
        ):
            raise RegistryError(f"guard shape: {type_name} exposes a reset/replace operation")
        atomic_methods = set(
            re.findall(r"self\s*\.\s*0\s*\.\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(", impl_text)
        )
        unexpected_atomic_methods = atomic_methods - {"load", "fetch_or"}
        if unexpected_atomic_methods:
            raise RegistryError(
                f"guard shape: {type_name} has non-monotone atomic operations "
                f"{sorted(unexpected_atomic_methods)}"
            )
        return
    raise RegistryError(f"guard shape: cannot locate private mask {type_name}")


def reject_mask_resets(path: pathlib.Path, field_names: set[str]) -> None:
    masked = strip_rust_comments_and_literals(path.read_text(encoding="utf-8"))
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        raise RegistryError(f"guard shape: invalid Rust structure in {path.relative_to(ROOT)}")
    tokens, _ = tokenized
    resets = sorted(
        field
        for index, token in enumerate(tokens[:-1])
        if (field := token.value.removeprefix("r#")) in field_names
        and index > 0
        and tokens[index - 1].value == "."
        and tokens[index + 1].value == "="
    )
    if resets:
        raise RegistryError(f"guard shape: monotone mask reset/replace forbidden {resets}")


def validate_bounded_guard_shapes() -> None:
    edge_path = ROOT / "src/strategies/binary_oracle_edge_taker/mod.rs"
    entry_path = ROOT / "src/strategies/binary_oracle_edge_taker/entry_decision.rs"
    maker_path = ROOT / "src/strategies/binary_oracle_maker/mod.rs"
    edge_fields = rust_named_struct_fields(edge_path, "BinaryOracleEdgeTaker")
    maker_fields = rust_named_struct_fields(maker_path, "BinaryOracleMaker")
    expected_edge = {
        "blocked_rv_novelty": ("LegacyBlockedRvNoveltyMask",),
        "entry_skip_novelty": ("LegacyEntrySkipNoveltyMask",),
        "last_recorded_exit_decision": ("Option", "<", "ExitDecisionDedupeKey", ">"),
        "last_exit_evidence_outcome": (
            "Option", "<", "(", "PositionId", ",", "ExitOutcomeKey", ")", ">"
        ),
    }
    expected_maker = {
        "requote_throttle_novelty": ("LegacyRequoteThrottleNoveltyMask",),
    }
    for field, expected_type in expected_edge.items():
        if edge_fields.get(field) != expected_type:
            raise RegistryError(
                f"guard shape: edge field {field} must have exact type {expected_type}"
            )
    for field, expected_type in expected_maker.items():
        if maker_fields.get(field) != expected_type:
            raise RegistryError(
                f"guard shape: maker field {field} must have exact type {expected_type}"
            )
    edge_source = edge_path.read_text(encoding="utf-8")
    maker_source = maker_path.read_text(encoding="utf-8")
    for field in ("blocked_rv_novelty", "entry_skip_novelty"):
        if not re.search(rf"#\[cfg\(test\)\]\s*{field}\s*:", edge_source):
            raise RegistryError(f"guard shape: {field} instance storage must remain test-only")
    if not re.search(
        r"#\[cfg\(test\)\]\s*requote_throttle_novelty\s*:", maker_source
    ):
        raise RegistryError(
            "guard shape: requote_throttle_novelty instance storage must remain test-only"
        )
    guarded_fragments = {
        "blocked_strategy_input": {"blocked_rv_novelty"},
        "entry_skip": {"entry_skip_novelty"},
        "requote_throttle": {"requote_throttle_novelty"},
    }
    all_fields = set(edge_fields) | set(maker_fields)
    for fragment, allowed in guarded_fragments.items():
        unexpected = sorted(field for field in all_fields if fragment in field and field not in allowed)
        if unexpected:
            raise RegistryError(
                f"guard shape: parallel or aliased {fragment} state forbidden {unexpected}"
            )
    masks = (
        (entry_path, "LegacyBlockedRvNoveltyMask", "BLOCKED_RV_NOVELTY", 12),
        (entry_path, "LegacyEntrySkipNoveltyMask", "ENTRY_SKIP_NOVELTY", 16),
        (maker_path, "LegacyRequoteThrottleNoveltyMask", "REQUOTE_THROTTLE_NOVELTY", 12),
    )
    for path, type_name, static_name, cardinality in masks:
        validate_private_u16_newtype(path, type_name, static_name, cardinality)
    reject_mask_resets(edge_path, {"blocked_rv_novelty", "entry_skip_novelty"})
    reject_mask_resets(maker_path, {"requote_throttle_novelty"})
    strategy_sources = edge_source + maker_source
    if "EvidenceEpisodeId" in strategy_sources:
        raise RegistryError("guard shape: incomplete episode identity cannot enable suppression")


def verify(path: pathlib.Path) -> None:
    data = load_registry(path)
    validate_header_contract(data)
    source_path = ROOT / data["source_module"]
    source = source_path.read_text(encoding="utf-8")
    allocations = validate_families(data)
    readers = validate_readers(data)
    validate_producers(data, source, allocations, readers)
    validate_handler_append_sweep(data)
    validate_identity_type(data)
    validate_bounded_guard_shapes()


def main() -> int:
    global ROOT
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry", type=pathlib.Path, default=DEFAULT_REGISTRY)
    parser.add_argument("--root", type=pathlib.Path)
    args = parser.parse_args()
    if args.root is not None:
        ROOT = args.root.resolve()
    try:
        verify(args.registry)
    except (RegistryError, OSError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: bolt-v3 evidence registry verified")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
