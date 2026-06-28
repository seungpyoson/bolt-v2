#!/usr/bin/env python3
"""Verify broad exception handlers do not hide fail-closed contract failures."""

from __future__ import annotations

import argparse
import ast
import fnmatch
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, cast


REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONFIG = Path("ci/fail-closed-contracts.toml")
CONFIG_TABLE = "fail_closed_contracts"
VALID_EXCEPTION_CLASSIFICATIONS = frozenset({"classified_degradation"})
REQUIRED_EXCEPTION_KEYS = frozenset({"path", "line", "rule_id", "classification", "reason"})


@dataclass(frozen=True)
class Config:
    include_globs: tuple[str, ...]
    exclude_globs: tuple[str, ...]
    broad_exception_names: frozenset[str]
    logging_call_names: frozenset[str]
    sentinel_return_shapes: frozenset[str]
    rule_ids: dict[str, str]
    exceptions: frozenset[tuple[str, int, str, str]]
    config_findings: tuple[str, ...]


@dataclass(frozen=True)
class HandlerFacts:
    rel_path: str
    line: int
    exception_names: frozenset[str]
    is_bare: bool
    only_pass: bool
    has_logging: bool
    sentinel_returns: frozenset[str]

    @property
    def is_broad(self) -> bool:
        return bool(self.exception_names)


@dataclass(frozen=True)
class Rule:
    key: str
    message: str
    applies: Callable[[HandlerFacts], bool]


RULES: tuple[Rule, ...] = (
    Rule("bare_except_pass", "bare except handler passes silently", lambda facts: facts.is_bare and facts.only_pass),
    Rule("broad_except_pass", "broad exception handler passes silently", lambda facts: facts.is_broad and facts.only_pass),
    Rule(
        "broad_logged_sentinel_return",
        "broad exception handler logs then returns a sentinel",
        lambda facts: facts.is_broad and facts.has_logging and bool(facts.sentinel_returns),
    ),
    Rule(
        "broad_sentinel_return",
        "broad exception handler returns a sentinel",
        lambda facts: facts.is_broad and not facts.has_logging and bool(facts.sentinel_returns),
    ),
)


def strings(value: object) -> tuple[str, ...]:
    return cast(tuple[str, ...], tuple(cast(Iterable[object], value)))


def non_empty_string(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip())


def central_exception_valid(entry: object) -> bool:
    return (
        isinstance(entry, dict)
        and frozenset(entry) == REQUIRED_EXCEPTION_KEYS
        and non_empty_string(entry["path"])
        and isinstance(entry["line"], int)
        and non_empty_string(entry["rule_id"])
        and entry["classification"] in VALID_EXCEPTION_CLASSIFICATIONS
        and non_empty_string(entry["reason"])
    )


def central_exception_record(entry: dict[str, object]) -> tuple[str, int, str, str]:
    return (
        cast(str, entry["path"]),
        cast(int, entry["line"]),
        cast(str, entry["rule_id"]),
        cast(str, entry["classification"]),
    )


def central_exception_finding(path: Path, rule_id: str, index: int) -> str:
    return (
        f"{rule_id}:{path.as_posix()}: exceptions[{index}] must declare "
        "a supported classification and non-empty reason"
    )


def central_exception_records(
    entries: tuple[object, ...], config_findings: tuple[str, ...]
) -> frozenset[tuple[str, int, str, str]]:
    if config_findings:
        return frozenset()
    return frozenset(
        central_exception_record(cast(dict[str, object], entry)) for entry in entries
    )


def load_config(path: Path) -> Config:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    table = data[CONFIG_TABLE]
    rule_ids = dict(table["rule_ids"])
    exception_entries = tuple(table["exceptions"])
    config_findings = tuple(
        central_exception_finding(path, str(rule_ids["central_exception_invalid"]), index)
        for index, entry in enumerate(exception_entries)
        if not central_exception_valid(entry)
    )
    return Config(
        include_globs=strings(table["include_globs"]),
        exclude_globs=strings(table["exclude_globs"]),
        broad_exception_names=frozenset(strings(table["broad_exception_names"])),
        logging_call_names=frozenset(strings(table["logging_call_names"])),
        sentinel_return_shapes=frozenset(strings(table["sentinel_return_shapes"])),
        rule_ids=rule_ids,
        exceptions=central_exception_records(exception_entries, config_findings),
        config_findings=config_findings,
    )


def rel_name(path: Path, root: Path) -> str:
    return path.relative_to(root).as_posix()


def selected_paths(root: Path, config: Config) -> list[Path]:
    included = {
        path
        for pattern in config.include_globs
        for path in root.glob(pattern)
        if path.is_file()
    }
    return sorted(
        path
        for path in included
        if not any(fnmatch.fnmatch(rel_name(path, root), pattern) for pattern in config.exclude_globs)
    )


def dotted_name(node: ast.AST) -> str | None:
    match node:
        case ast.Name(id=name):
            return name
        case ast.Attribute(value=value, attr=attr):
            base = dotted_name(value)
            match base:
                case str():
                    return ".".join((base, attr))
                case None:
                    return None
        case _:
            return None


def exception_name_candidates(node: ast.AST) -> frozenset[str]:
    match node:
        case ast.Name(id=name):
            return frozenset((name,))
        case ast.Attribute():
            match dotted_name(node):
                case str(name):
                    return frozenset((name,))
                case None:
                    return frozenset()
        case ast.Tuple(elts=elts):
            return frozenset().union(*(exception_name_candidates(item) for item in elts))
        case ast.IfExp(body=body, orelse=orelse):
            return exception_name_candidates(body) | exception_name_candidates(orelse)
        case _:
            return frozenset()


def exception_names(handler: ast.ExceptHandler, broad_names: frozenset[str]) -> frozenset[str]:
    match handler.type:
        case None:
            candidates = frozenset()
        case _:
            candidates = exception_name_candidates(handler.type)
    return frozenset(
        name for name in candidates if name.rsplit(".", maxsplit=1)[-1] in broad_names
    )


def call_names(node: ast.AST) -> Iterable[str]:
    for child in ast.walk(node):
        if isinstance(child, ast.Call) and (name := dotted_name(child.func)):
            yield name


def sentinel_shape(node: ast.AST) -> str | None:
    match node:
        case ast.Constant(value=None):
            return "none"
        case ast.List(elts=[]):
            return "empty_list"
        case ast.Dict(keys=[], values=[]):
            return "empty_dict"
        case ast.Constant(value=""):
            return "empty_string"
        case ast.Constant(value=False):
            return "false"
        case _:
            return None


def sentinel_returns(handler: ast.ExceptHandler, configured_shapes: frozenset[str]) -> frozenset[str]:
    return frozenset(
        shape
        for node in ast.walk(handler)
        if isinstance(node, ast.Return)
        if (shape := sentinel_shape(node.value)) in configured_shapes
    )


def facts_for_handler(rel_path: str, handler: ast.ExceptHandler, config: Config) -> HandlerFacts:
    return HandlerFacts(
        rel_path=rel_path,
        line=handler.lineno,
        exception_names=exception_names(handler, config.broad_exception_names),
        is_bare=handler.type is None,
        only_pass=len(handler.body) == 1 and isinstance(handler.body[0], ast.Pass),
        has_logging=bool(config.logging_call_names.intersection(call_names(handler))),
        sentinel_returns=sentinel_returns(handler, config.sentinel_return_shapes),
    )


def scan_file(root: Path, path: Path, config: Config) -> list[HandlerFacts]:
    rel_path = rel_name(path, root)
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=rel_path)
    return [
        facts_for_handler(rel_path, node, config)
        for node in ast.walk(tree)
        if isinstance(node, ast.ExceptHandler)
    ]


def allowed(config: Config, facts: HandlerFacts, rule_id: str) -> bool:
    return bool(
        {
            (facts.rel_path, facts.line, rule_id, classification)
            for classification in VALID_EXCEPTION_CLASSIFICATIONS
        }.intersection(config.exceptions)
    )


def finding(config: Config, facts: HandlerFacts, rule: Rule) -> str | None:
    rule_id = config.rule_ids[rule.key]
    if allowed(config, facts, rule_id):
        return None
    return f"{rule_id}:{facts.rel_path}:{facts.line}: {rule.message}"


def findings_for_facts(config: Config, facts: HandlerFacts) -> list[str]:
    return [
        result
        for rule in RULES
        if rule.applies(facts)
        if (result := finding(config, facts, rule)) is not None
    ]


def collect_findings(root: Path, config_path: Path = REPO_ROOT / DEFAULT_CONFIG) -> list[str]:
    root = root.resolve()
    config = load_config(config_path)
    return list(config.config_findings) + [
        finding
        for path in selected_paths(root, config)
        for facts in scan_file(root, path, config)
        for finding in findings_for_facts(config, facts)
    ]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--config", type=Path, default=REPO_ROOT / DEFAULT_CONFIG)
    args = parser.parse_args(argv)

    findings = collect_findings(args.root, args.config)
    if findings:
        print("FAIL: fail-closed contract violations:", file=sys.stderr)
        for item in findings:
            print(f"  {item}", file=sys.stderr)
        return 1
    print("OK: fail-closed contract verifier passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
