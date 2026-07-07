#!/usr/bin/env python3
"""Verify broad exception handlers do not hide fail-closed contract failures."""

from __future__ import annotations

import argparse
import ast
import fnmatch
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, cast

from verifier_io import require_nonempty


REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONFIG = Path("ci/fail-closed-contracts.toml")
DEFAULT_EXCEPTIONS_CONFIG = Path("ci/fail-closed-exceptions.toml")
JUSTFILE = Path("justfile")
CONFIG_TABLE = "fail_closed_contracts"
EXCEPTIONS_TABLE = "fail_closed_exceptions"
SUPPORTED_CONFIG_VERSION = 1
SOURCE_FENCE_STATIC_RECIPE = "source-fence-static-inner"
REQUIRED_SOURCE_FENCE_COMMANDS = (
    "python3 scripts/run_fences.py",
)


@dataclass(frozen=True)
class Config:
    include_globs: tuple[str, ...]
    exclude_globs: tuple[str, ...]
    broad_exception_names: frozenset[str]
    logging_call_names: frozenset[str]
    rule_ids: dict[str, str]


@dataclass(frozen=True)
class HandlerFacts:
    rel_path: str
    line: int
    exception_names: frozenset[str]
    is_bare: bool
    is_silent: bool
    has_logging: bool
    sentinel_returns: frozenset[str]

    @property
    def is_broad(self) -> bool:
        return bool(self.exception_names)

    @property
    def catches_all(self) -> bool:
        return self.is_bare or self.is_broad


@dataclass(frozen=True)
class ExceptionKey:
    rule_id: str
    path: str
    line: int


@dataclass(frozen=True)
class Exceptions:
    entries: dict[ExceptionKey, str]


@dataclass(frozen=True)
class Rule:
    key: str
    message: str
    applies: Callable[[HandlerFacts], bool]


RULES: tuple[Rule, ...] = (
    Rule("bare_except_pass", "bare except handler passes silently", lambda facts: facts.is_bare and facts.is_silent),
    Rule("broad_except_pass", "broad exception handler passes silently", lambda facts: facts.is_broad and facts.is_silent),
    Rule(
        "broad_logged_sentinel_return",
        "catch-all exception handler logs then returns a sentinel",
        lambda facts: facts.catches_all and facts.has_logging and bool(facts.sentinel_returns),
    ),
    Rule(
        "broad_sentinel_return",
        "catch-all exception handler returns a sentinel",
        lambda facts: facts.catches_all and not facts.has_logging and bool(facts.sentinel_returns),
    ),
)
NESTED_SCOPE_NODES = (
    ast.AsyncFunctionDef,
    ast.ClassDef,
    ast.ExceptHandler,
    ast.FunctionDef,
    ast.Lambda,
)

CONFIG_KEYS = frozenset(
    {
        "version",
        "include_globs",
        "exclude_globs",
        "broad_exception_names",
        "logging_call_names",
        "rule_ids",
    }
)
RULE_ID_KEYS = frozenset(rule.key for rule in RULES)
EXCEPTIONS_KEYS = frozenset({"version", "exceptions"})
EXCEPTION_ENTRY_KEYS = frozenset({"rule_id", "path", "line", "reason"})
STALE_EXCEPTION_RULE_ID = "FLC000"
LOGGER_RECEIVER_NAMES = frozenset({"log", "logger", "logging"})
LOGGER_FACTORY_NAMES = frozenset({"get_log", "get_logger", "get_logging", "getLogger"})


def strings(field_name: str, value: object) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise TypeError(f"{field_name} must be a list of strings")
    if not all(isinstance(item, str) for item in value):
        raise TypeError(f"{field_name} must contain only strings")
    return tuple(cast(list[str], value))


def string_map(field_name: str, value: object) -> dict[str, str]:
    if not isinstance(value, dict):
        raise TypeError(f"{field_name} must be a string-to-string table")
    entries = tuple(value.items())
    if not all(isinstance(key, str) and isinstance(item, str) for key, item in entries):
        raise TypeError(f"{field_name} must contain only string keys and values")
    return dict(cast(tuple[tuple[str, str], ...], entries))


def require_keys(field_name: str, actual: frozenset[str], expected: frozenset[str]) -> None:
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    if missing or extra:
        raise TypeError(f"{field_name} keys must be exactly {sorted(expected)}")


def require_version(field_name: str, value: object) -> None:
    if not isinstance(value, int) or isinstance(value, bool) or value != SUPPORTED_CONFIG_VERSION:
        raise TypeError(f"{field_name} must be {SUPPORTED_CONFIG_VERSION}")


def positive_int(field_name: str, value: object) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise TypeError(f"{field_name} must be a positive integer")
    return value


def nonempty_string(field_name: str, value: object) -> str:
    if not isinstance(value, str) or not value.strip():
        raise TypeError(f"{field_name} must be a non-empty string")
    return value


def relative_repo_path(field_name: str, value: object) -> str:
    text = nonempty_string(field_name, value)
    path = Path(text)
    if path.is_absolute() or ".." in path.parts:
        raise TypeError(f"{field_name} must be a repository-relative path")
    return path.as_posix()


def load_config(path: Path) -> Config:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    table = data[CONFIG_TABLE]
    require_keys(CONFIG_TABLE, frozenset(table), CONFIG_KEYS)
    require_version("version", table["version"])
    rule_ids = string_map("rule_ids", table["rule_ids"])
    require_keys("rule_ids", frozenset(rule_ids), RULE_ID_KEYS)
    return Config(
        include_globs=strings("include_globs", table["include_globs"]),
        exclude_globs=strings("exclude_globs", table["exclude_globs"]),
        broad_exception_names=frozenset(strings("broad_exception_names", table["broad_exception_names"])),
        logging_call_names=frozenset(strings("logging_call_names", table["logging_call_names"])),
        rule_ids=rule_ids,
    )


def load_exceptions(path: Path, valid_rule_ids: frozenset[str]) -> Exceptions:
    if not path.exists():
        return Exceptions({})
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    table = data[EXCEPTIONS_TABLE]
    require_keys(EXCEPTIONS_TABLE, frozenset(table), EXCEPTIONS_KEYS)
    require_version("exceptions version", table["version"])
    entries_raw = table["exceptions"]
    if not isinstance(entries_raw, list):
        raise TypeError("exceptions must be a list of tables")
    entries: dict[ExceptionKey, str] = {}
    for item in entries_raw:
        if not isinstance(item, dict):
            raise TypeError("exceptions must contain only tables")
        require_keys("exception entry", frozenset(item), EXCEPTION_ENTRY_KEYS)
        rule_id = nonempty_string("exception rule_id", item["rule_id"])
        if rule_id not in valid_rule_ids:
            raise TypeError(f"exception rule_id must be one of {sorted(valid_rule_ids)}")
        key = ExceptionKey(
            rule_id=rule_id,
            path=relative_repo_path("exception path", item["path"]),
            line=positive_int("exception line", item["line"]),
        )
        if key in entries:
            raise TypeError(f"duplicate fail-closed exception for {key.path}:{key.line}:{key.rule_id}")
        entries[key] = nonempty_string("exception reason", item["reason"])
    return Exceptions(entries)


def rel_name(path: Path, root: Path) -> str:
    return path.relative_to(root).as_posix()


def strip_just_comment(line: str) -> str:
    quote: str | None = None
    escaped = False
    for index, char in enumerate(line):
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\" and quote == '"':
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"'}:
            quote = char
            continue
        if char == "#" and (index == 0 or line[index - 1].isspace()):
            return line[:index].rstrip()
    return line.rstrip()


def is_just_recipe_header(line: str) -> bool:
    stripped = line.strip()
    if not stripped or line != line.lstrip() or stripped.startswith("#") or ":=" in stripped:
        return False
    header, separator, _tail = stripped.partition(":")
    if not separator:
        return False
    parts = header.split()
    return bool(parts) and re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]*", parts[0]) is not None


def just_recipe_name(line: str) -> str:
    return line.strip().partition(":")[0].split()[0]


def source_fence_static_commands(justfile_text: str) -> tuple[str, ...]:
    commands: list[str] = []
    active = False
    for line in justfile_text.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        if line == line.lstrip():
            active = False
            if is_just_recipe_header(line):
                active = just_recipe_name(line) == SOURCE_FENCE_STATIC_RECIPE
        elif active and not stripped.startswith("#"):
            command = strip_just_comment(stripped)
            if command:
                commands.append(command)
    return tuple(commands)


def source_fence_wiring_findings(root: Path) -> list[str]:
    try:
        justfile_text = (root / JUSTFILE).read_text(encoding="utf-8")
    except FileNotFoundError:
        return [f"{SOURCE_FENCE_STATIC_RECIPE} recipe missing from {JUSTFILE}"]
    commands = source_fence_static_commands(justfile_text)
    if commands != REQUIRED_SOURCE_FENCE_COMMANDS:
        expected = " && ".join(REQUIRED_SOURCE_FENCE_COMMANDS)
        return [f"{SOURCE_FENCE_STATIC_RECIPE} must contain only {expected}"]
    return []


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
    return frozenset(
        name
        for child in ast.walk(node)
        if (name := dotted_name(child)) is not None
    )


def exception_names(handler: ast.ExceptHandler, broad_names: frozenset[str]) -> frozenset[str]:
    match handler.type:
        case None:
            candidates = frozenset()
        case _:
            candidates = exception_name_candidates(handler.type)
    builtins_names = frozenset(f"builtins.{name}" for name in broad_names)
    return frozenset(
        name for name in candidates if name in broad_names or name in builtins_names
    )


def handler_body_walk(handler: ast.ExceptHandler) -> Iterable[ast.AST]:
    stack = list(reversed(handler.body))
    while stack:
        node = stack.pop()
        yield node
        if isinstance(node, NESTED_SCOPE_NODES):
            continue
        stack.extend(reversed(list(ast.iter_child_nodes(node))))


def is_logging_call(node: ast.AST, config: Config) -> bool:
    if not isinstance(node, ast.Call):
        return False
    name = dotted_name(node.func)
    if name is not None and name in config.logging_call_names:
        return True
    if not isinstance(node.func, ast.Attribute) or node.func.attr != "exception":
        return False
    return logger_receiver(node.func.value)


def logger_receiver(node: ast.AST) -> bool:
    name = dotted_name(node)
    if name is not None:
        return name.rsplit(".", maxsplit=1)[-1] in LOGGER_RECEIVER_NAMES
    if isinstance(node, ast.Call):
        call_name = dotted_name(node.func)
        if call_name is None:
            return False
        return call_name.rsplit(".", maxsplit=1)[-1] in LOGGER_FACTORY_NAMES
    return False


def sentinel_shape(node: ast.AST | None) -> str | None:
    match node:
        case None:
            return "none"
        case ast.Constant(value=None):
            return "none"
        case ast.List(elts=[]):
            return "empty_list"
        case ast.Dict(keys=[], values=[]):
            return "empty_dict"
        case ast.Tuple(elts=[]):
            return "empty_tuple"
        case ast.Constant(value=""):
            return "empty_string"
        case ast.Constant(value=False):
            return "false"
        case _:
            return None


def sentinel_shapes(node: ast.AST | None) -> frozenset[str]:
    direct = sentinel_shape(node)
    if direct is not None:
        return frozenset((direct,))
    match node:
        case ast.IfExp(body=body, orelse=orelse):
            return sentinel_shapes(body) | sentinel_shapes(orelse)
        case ast.BoolOp(values=values):
            return frozenset().union(*(sentinel_shapes(value) for value in values))
        case ast.Tuple(elts=elts):
            return frozenset().union(*(sentinel_shapes(elt) for elt in elts))
        case _:
            return frozenset()


def sentinel_returns(handler: ast.ExceptHandler) -> frozenset[str]:
    return frozenset(
        shape
        for node in handler_body_walk(handler)
        if isinstance(node, ast.Return)
        for shape in sentinel_shapes(node.value)
    )


def is_silent_handler_statement(node: ast.AST) -> bool:
    match node:
        case ast.Pass():
            return True
        case ast.Expr(value=ast.Constant(value=Ellipsis)):
            return True
        case _:
            return False


def handler_body_is_silent(handler: ast.ExceptHandler) -> bool:
    return bool(handler.body) and all(is_silent_handler_statement(stmt) for stmt in handler.body)


def facts_for_handler(rel_path: str, handler: ast.ExceptHandler, config: Config) -> HandlerFacts:
    body_nodes = tuple(handler_body_walk(handler))
    return HandlerFacts(
        rel_path=rel_path,
        line=handler.lineno,
        exception_names=exception_names(handler, config.broad_exception_names),
        is_bare=handler.type is None,
        is_silent=handler_body_is_silent(handler),
        has_logging=any(is_logging_call(node, config) for node in body_nodes),
        sentinel_returns=sentinel_returns(handler),
    )


def scan_file(root: Path, path: Path, config: Config) -> list[HandlerFacts]:
    rel_path = rel_name(path, root)
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=rel_path)
    return [
        facts_for_handler(rel_path, node, config)
        for node in ast.walk(tree)
        if isinstance(node, ast.ExceptHandler)
    ]


def finding(config: Config, facts: HandlerFacts, rule: Rule) -> str | None:
    rule_id = config.rule_ids[rule.key]
    return f"{rule_id}:{facts.rel_path}:{facts.line}: {rule.message}"


def exception_key(config: Config, facts: HandlerFacts, rule: Rule) -> ExceptionKey:
    return ExceptionKey(
        rule_id=config.rule_ids[rule.key],
        path=facts.rel_path,
        line=facts.line,
    )


def raw_findings_for_facts(config: Config, facts: HandlerFacts) -> list[tuple[ExceptionKey, str]]:
    return [
        (exception_key(config, facts, rule), result)
        for rule in RULES
        if rule.applies(facts)
        if (result := finding(config, facts, rule)) is not None
    ]


def findings_for_facts(config: Config, facts: HandlerFacts) -> list[str]:
    return [result for _, result in raw_findings_for_facts(config, facts)]


def collect_findings(
    root: Path,
    config_path: Path = REPO_ROOT / DEFAULT_CONFIG,
    exceptions_path: Path | None = None,
) -> list[str]:
    root = root.resolve()
    config = load_config(config_path)
    exceptions = load_exceptions(
        exceptions_path or config_path.with_name(DEFAULT_EXCEPTIONS_CONFIG.name),
        frozenset(config.rule_ids.values()),
    )
    source_fence_findings = source_fence_wiring_findings(root)
    paths = selected_paths(root, config)
    path_findings: list[str] = []
    require_nonempty(paths, "fail-closed contract selected paths", path_findings)
    if path_findings:
        return source_fence_findings + path_findings
    raw_findings = [
        raw_finding
        for path in paths
        for facts in scan_file(root, path, config)
        for raw_finding in raw_findings_for_facts(config, facts)
    ]
    matched_exceptions: set[ExceptionKey] = set()
    findings: list[str] = []
    for key, text in raw_findings:
        if key in exceptions.entries:
            matched_exceptions.add(key)
        else:
            findings.append(text)
    stale = sorted(set(exceptions.entries) - matched_exceptions, key=lambda key: (key.path, key.line, key.rule_id))
    findings.extend(
        f"{STALE_EXCEPTION_RULE_ID}:{key.path}:{key.line}: stale fail-closed exception for {key.rule_id}"
        for key in stale
    )
    return source_fence_findings + path_findings + findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--config", type=Path, default=REPO_ROOT / DEFAULT_CONFIG)
    parser.add_argument("--exceptions-config", type=Path)
    args = parser.parse_args(argv)

    try:
        findings = collect_findings(args.root, args.config, args.exceptions_config)
    except (OSError, KeyError, SyntaxError, TypeError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        print(f"FAIL: fail-closed contract verifier input error: {exc}", file=sys.stderr)
        return 2
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
