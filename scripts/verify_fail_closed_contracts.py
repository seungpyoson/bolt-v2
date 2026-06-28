#!/usr/bin/env python3
"""Verify fail-closed Python exception contracts from explicit TOML policy."""

from __future__ import annotations

import argparse
import ast
import copy
import sys
import tomllib
from dataclasses import dataclass
from datetime import date
from itertools import groupby
from operator import attrgetter
from pathlib import Path
from typing import Any


JUSTFILE = Path("justfile")
DISPOSITIONS = frozenset(("always_block", "central_exception"))


def same_date(value: date) -> date:
    return value


DATE_READERS: dict[type, Any] = {
    date: same_date,
    str: date.fromisoformat,
}


class ContractError(ValueError):
    """Typed boundary-validation failure."""


class EventNormalizer(ast.NodeTransformer):
    """Emit stable handler keys without carrying call payload text."""

    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> ast.AST:
        return ast.copy_location(
            ast.ExceptHandler(
                type=copy.deepcopy(node.type),
                name=node.name,
                body=[self.visit(statement) for statement in node.body],
            ),
            node,
        )

    def visit_Call(self, node: ast.Call) -> ast.AST:
        return ast.copy_location(ast.Call(func=ast.Name(id="$call", ctx=ast.Load()), args=[], keywords=[]), node)


class ExceptionHandlerCollector(ast.NodeVisitor):
    def __init__(self) -> None:
        self.handlers: list[ast.ExceptHandler] = []

    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
        self.handlers.append(node)
        self.generic_visit(node)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractError(message)


@dataclass(frozen=True)
class Rule:
    id: str
    classification: str
    disposition: str
    event_keys: tuple[str, ...]


@dataclass(frozen=True)
class Contract:
    include: tuple[str, ...]
    exclude: tuple[str, ...]
    source_fence_commands: tuple[str, ...]
    rules: tuple[Rule, ...]


@dataclass(frozen=True)
class ExceptionRecord:
    rule_id: str
    path: str
    line: int
    classification: str
    expires_on: date
    reason: str

    @property
    def key(self) -> tuple[str, str, int, str]:
        return (self.rule_id, self.path, self.line, self.classification)


@dataclass(frozen=True)
class Exceptions:
    records: dict[tuple[str, str, int, str], ExceptionRecord]


@dataclass(frozen=True)
class Event:
    path: str
    line: int
    key: str


@dataclass(frozen=True)
class Violation:
    rule: Rule
    event: Event

    @property
    def exception_key(self) -> tuple[str, str, int, str]:
        return (self.rule.id, self.event.path, self.event.line, self.rule.classification)


def load_toml(path: Path, label: str) -> dict[str, Any]:
    try:
        text = path.read_text(encoding="utf-8")
        require(bool(text.strip()), f"{label} file empty: {path}")
        return tomllib.loads(text)
    except FileNotFoundError:
        raise ContractError(f"{label} file absent: {path}") from None
    except IsADirectoryError as exc:
        raise ContractError(f"{label} file unavailable: {path}: {exc}") from None
    except OSError as exc:
        raise ContractError(f"{label} file unavailable: {path}: {exc}") from None
    except tomllib.TOMLDecodeError as exc:
        raise ContractError(f"{label} file invalid: {path}: {exc}") from None


def value_at(data: dict[str, Any], path: tuple[str, ...], label: str) -> Any:
    value: Any = data
    for key in path:
        require(isinstance(value, dict), f"{label} invalid: missing {'.'.join(path)}")
        require(key in value, f"{label} invalid: missing {'.'.join(path)}")
        value = value[key]
    return value


def string_value(value: Any, label: str) -> str:
    require(isinstance(value, str) and bool(value), f"{label} must be a non-empty string")
    return value


def string_list(value: Any, label: str, *, allow_empty: bool) -> tuple[str, ...]:
    require(isinstance(value, list), f"{label} must be a string list")
    require(all(isinstance(item, str) and item for item in value), f"{label} must be a string list")
    items = tuple(value)
    require(allow_empty or bool(items), f"{label} must be non-empty")
    return items


def table_array(value: Any, label: str, *, allow_empty: bool) -> list[dict[str, Any]]:
    require(isinstance(value, list), f"{label} must be a table array")
    require(allow_empty or bool(value), f"{label} must be non-empty")
    require(all(isinstance(item, dict) for item in value), f"{label} must contain only tables")
    return value


def date_value(value: Any, label: str) -> date:
    try:
        return DATE_READERS[type(value)](value)
    except KeyError:
        raise ContractError(f"{label} must be ISO date") from None
    except ValueError:
        raise ContractError(f"{label} must be ISO date") from None


def positive_int(value: Any, label: str) -> int:
    require(isinstance(value, int) and value > 0, f"{label} must be a positive integer")
    return value


def rule_from(raw: dict[str, Any]) -> Rule:
    rule_id = string_value(value_at(raw, ("id",), "contract rule"), "contract rule id")
    disposition = string_value(value_at(raw, ("disposition",), f"contract rule {rule_id}"), f"contract rule {rule_id}.disposition")
    require(disposition in DISPOSITIONS, f"contract rule {rule_id}.disposition must be one of {sorted(DISPOSITIONS)}")
    return Rule(
        id=rule_id,
        classification=string_value(value_at(raw, ("classification",), f"contract rule {rule_id}"), f"contract rule {rule_id}.classification"),
        disposition=disposition,
        event_keys=string_list(value_at(raw, ("event_keys",), f"contract rule {rule_id}"), f"contract rule {rule_id}.event_keys", allow_empty=False),
    )


def parse_rules(data: dict[str, Any]) -> tuple[Rule, ...]:
    rules = tuple(rule_from(raw) for raw in table_array(value_at(data, ("rules",), "contract file"), "contract file rules", allow_empty=False))
    ids = tuple(rule.id for rule in rules)
    declared_keys = tuple(declared_key for rule in rules for declared_key in rule.event_keys)
    require(len(ids) == len(set(ids)), "contract file invalid: duplicate/ambiguous rule id")
    require(len(declared_keys) == len(set(declared_keys)), "contract file invalid: duplicate/ambiguous rule event key")
    return rules


def parse_contract(data: dict[str, Any]) -> Contract:
    return Contract(
        include=string_list(value_at(data, ("scan", "include"), "contract file"), "contract scan.include", allow_empty=False),
        exclude=string_list(value_at(data, ("scan", "exclude"), "contract file"), "contract scan.exclude", allow_empty=True),
        source_fence_commands=string_list(value_at(data, ("source_fence", "commands"), "contract file"), "contract source_fence.commands", allow_empty=False),
        rules=parse_rules(data),
    )


def exception_record(raw: dict[str, Any], index: int) -> ExceptionRecord:
    label = f"exceptions.items[{index}]"
    return ExceptionRecord(
        rule_id=string_value(value_at(raw, ("rule_id",), "exceptions file"), f"{label}.rule_id"),
        path=string_value(value_at(raw, ("path",), "exceptions file"), f"{label}.path"),
        line=positive_int(value_at(raw, ("line",), "exceptions file"), f"{label}.line"),
        classification=string_value(value_at(raw, ("classification",), "exceptions file"), f"{label}.classification"),
        expires_on=date_value(value_at(raw, ("expires_on",), "exceptions file"), f"{label}.expires_on"),
        reason=string_value(value_at(raw, ("reason",), "exceptions file"), f"{label}.reason"),
    )


def parse_exceptions(data: dict[str, Any]) -> Exceptions:
    raw_items = table_array(value_at(data, ("exceptions", "items"), "exceptions file"), "exceptions file items", allow_empty=True)
    records: dict[tuple[str, str, int, str], ExceptionRecord] = {}
    for index, raw in enumerate(raw_items):
        record = exception_record(raw, index)
        require(record.key not in records, f"exceptions file invalid: duplicate/ambiguous exception {record.key}")
        records[record.key] = record
    return Exceptions(records=records)


def scan_files(root: Path, contract: Contract) -> tuple[Path, ...]:
    excluded = {path.resolve() for pattern in contract.exclude for path in root.glob(pattern)}
    candidates = {path.resolve() for pattern in contract.include for path in root.glob(pattern)}
    files = tuple(sorted(set(filter(Path.is_file, candidates)) - excluded))
    require(bool(files), "contract file invalid: scan.include matched no Python files")
    return files


def parse_python(path: Path) -> ast.AST:
    try:
        return ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except SyntaxError as exc:
        raise ContractError(f"source file invalid: {path}: {exc}") from None
    except OSError as exc:
        raise ContractError(f"source file unavailable: {path}: {exc}") from None


def event_key(handler: ast.ExceptHandler) -> str:
    normalized = EventNormalizer().visit(copy.deepcopy(handler))
    return ast.dump(normalized, include_attributes=False)


def events_for_file(root: Path, path: Path) -> tuple[Event, ...]:
    rel_path = path.relative_to(root).as_posix()
    collector = ExceptionHandlerCollector()
    collector.visit(parse_python(path))
    return tuple(
        Event(
            path=rel_path,
            line=node.lineno,
            key=event_key(node),
        )
        for node in collector.handlers
    )


def source_fence_findings(root: Path, contract: Contract) -> list[str]:
    try:
        text = (root / JUSTFILE).read_text(encoding="utf-8")
    except FileNotFoundError:
        return [f"source-fence command mismatch: {JUSTFILE} is absent"]
    except OSError as exc:
        return [f"source-fence command mismatch: {JUSTFILE} is unavailable: {exc}"]
    missing = [command for command in contract.source_fence_commands if command not in text]
    return [f"source-fence command mismatch: {JUSTFILE} missing `{command}`" for command in missing]


def violations_for_events(events: tuple[Event, ...], rules: tuple[Rule, ...]) -> list[Violation]:
    rule_by_key = {declared_key: rule for rule in rules for declared_key in rule.event_keys}
    events_by_key = {
        scanned_key: tuple(group)
        for scanned_key, group in groupby(sorted(events, key=attrgetter("key")), key=attrgetter("key"))
    }
    matching_keys = sorted(set(rule_by_key).intersection(events_by_key))
    return [
        Violation(rule=rule, event=event)
        for event_key in matching_keys
        for rule in (rule_by_key[event_key],)
        for event in events_by_key[event_key]
    ]


def finding_for_blocked(violation: Violation) -> str:
    return f"{violation.rule.id} {violation.event.path}:{violation.event.line} blocked by fail-closed contract"


def finding_for_missing(violation: Violation) -> str:
    return f"{violation.rule.id} {violation.event.path}:{violation.event.line} missing central exception for {violation.rule.classification}"


def finding_for_expired(violation: Violation, record: ExceptionRecord) -> str:
    return f"{violation.rule.id} {violation.event.path}:{violation.event.line} stale/expired exception expired on {record.expires_on.isoformat()}"


def reconcile(violations: list[Violation], exceptions: Exceptions, today: date) -> list[str]:
    always_block = tuple(violation for violation in violations if violation.rule.disposition == "always_block")
    central = {
        violation.exception_key: violation
        for violation in violations
        if violation.rule.disposition == "central_exception"
    }
    exception_keys = set(exceptions.records)
    expired = {key for key, record in exceptions.records.items() if record.expires_on < today}
    central_keys = set(central)
    missing = central_keys - exception_keys
    expired_required = central_keys.intersection(expired)
    unmatched = exception_keys - central_keys
    return (
        [finding_for_blocked(violation) for violation in always_block]
        + [finding_for_missing(central[key]) for key in sorted(missing)]
        + [finding_for_expired(central[key], exceptions.records[key]) for key in sorted(expired_required)]
        + [f"exceptions file invalid: unmatched central exception {key}" for key in sorted(unmatched)]
    )


def scan_contract(root: Path, contract: Contract, exceptions: Exceptions, today: date) -> list[str]:
    events = tuple(event for path in scan_files(root, contract) for event in events_for_file(root, path))
    return source_fence_findings(root, contract) + reconcile(violations_for_events(events, contract.rules), exceptions, today)


def scan_root(root: Path, *, contract_path: Path, exceptions_path: Path, today: date) -> list[str]:
    try:
        contract = parse_contract(load_toml(contract_path, "contract"))
        exceptions = parse_exceptions(load_toml(exceptions_path, "exceptions"))
        return scan_contract(root.resolve(), contract, exceptions, today)
    except ContractError as exc:
        return [str(exc)]


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--contract", type=Path, required=True)
    parser.add_argument("--exceptions", type=Path, required=True)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    findings = scan_root(
        Path.cwd(),
        contract_path=Path.cwd() / args.contract,
        exceptions_path=Path.cwd() / args.exceptions,
        today=date.today(),
    )
    require(not findings, "\n".join(findings))
    print("OK: fail-closed contract verifier passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    try:
        raise SystemExit(main())
    except ContractError as exc:
        print("FAIL: fail-closed contract violations:", file=sys.stderr)
        for finding in str(exc).splitlines():
            print(f"  {finding}", file=sys.stderr)
        raise SystemExit(1)
