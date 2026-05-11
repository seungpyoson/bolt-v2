#!/usr/bin/env python3
"""Audit Bolt-v3 provider-specific leakage in core-adjacent boundaries.

Provider-specific NT types, provider-key literals, and concrete provider
dispatch belong in `src/bolt_v3_providers/<provider>.rs`, not in the core
Bolt-v3 assembly files. This verifier is intentionally strict over production
code and ignores comments plus `#[cfg(test)] mod tests` fixtures.
"""

from __future__ import annotations

import argparse
import re
import string
import sys
from dataclasses import dataclass, field
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    message: str
    excerpt: str

    def render(self, prefix: str) -> str:
        return f"{prefix}: {self.path}:{self.line}: {self.message}: {self.excerpt}"


@dataclass(frozen=True)
class Rule:
    path: str
    pattern: re.Pattern[str]
    message: str


@dataclass(frozen=True)
class FindingAllowance:
    path: str
    message: str
    exact_excerpt: str


FINDING_ALLOWANCES = ()


def rules_for(
    paths: tuple[str, ...],
    pattern: re.Pattern[str],
    message: str,
) -> list[Rule]:
    return [Rule(path, pattern, message) for path in paths]


def discovered_core_files(root: Path) -> tuple[str, ...]:
    """Return core Bolt-v3 files; binding modules are intentionally excluded."""

    src = root / "src"
    paths = set(src.glob("bolt_v3_*.rs"))
    paths.update(
        path
        for path in (
            src / "bolt_v3_archetypes" / "mod.rs",
            src / "bolt_v3_market_families" / "mod.rs",
            src / "bolt_v3_providers" / "mod.rs",
        )
        if path.exists()
    )
    return tuple(sorted(path.relative_to(root).as_posix() for path in paths))


def discovered_binding_names(root: Path, directory: str) -> tuple[str, ...]:
    binding_dir = root / "src" / directory
    if not binding_dir.exists():
        return ()

    names = {path.stem for path in binding_dir.glob("*.rs") if path.name != "mod.rs"}
    names.update(path.name for path in binding_dir.iterdir() if path.is_dir())
    return tuple(sorted(names))


def discovered_provider_binding_files(root: Path) -> tuple[str, ...]:
    binding_dir = root / "src" / "bolt_v3_providers"
    if not binding_dir.exists():
        return ()

    return tuple(
        sorted(
            path.relative_to(root).as_posix()
            for path in binding_dir.glob("*.rs")
            if path.name != "mod.rs"
        )
    )


def snake_to_pascal(name: str) -> str:
    return "".join(part[:1].upper() + part[1:] for part in name.split("_") if part)


def alternation(names: tuple[str, ...]) -> str:
    if not names:
        return r"(?!)"
    return "|".join(re.escape(name) for name in names)


def rules_for_root(root: Path) -> list[Rule]:
    core_files = discovered_core_files(root)
    provider_names = discovered_binding_names(root, "bolt_v3_providers")
    family_names = discovered_binding_names(root, "bolt_v3_market_families")
    provider_alt = alternation(provider_names)
    provider_type_alt = alternation(tuple(snake_to_pascal(name) for name in provider_names))
    family_alt = alternation(family_names)
    family_type_alt = alternation(tuple(snake_to_pascal(name) for name in family_names))
    provider_binding_files = discovered_provider_binding_files(root)

    return [
        *rules_for(
            core_files,
            re.compile(rf"\bnautilus_(?:{provider_alt})::"),
            "concrete NT provider crate in core production code",
        ),
        *rules_for(
            core_files,
            re.compile(rf"\b[A-Za-z0-9_]*(?:{provider_type_alt})[A-Za-z0-9_]*\b"),
            "concrete provider type name in core production code",
        ),
        *rules_for(
            core_files,
            re.compile(
                rf"\b(?:pub\s+use|use)\s+crate::bolt_v3_providers::[^;]*"
                rf"\b(?:{provider_alt})\b",
                re.DOTALL,
            ),
            "core imports or re-exports concrete provider module",
        ),
        *rules_for(
            core_files,
            re.compile(rf"\bbolt_v3_providers::(?:{provider_alt})::"),
            "core accesses concrete provider module path",
        ),
        *rules_for(
            core_files,
            re.compile(rf'"(?:{provider_alt})"'),
            "provider-key string literal in core production code",
        ),
        *rules_for(
            core_files,
            re.compile(rf'"(?:{family_alt})"'),
            "market-family key string literal in core production code",
        ),
        *rules_for(
            core_files,
            re.compile(rf"\bbolt_v3_market_families::(?:{family_alt})::"),
            "core accesses concrete market-family module path",
        ),
        *rules_for(
            core_files,
            re.compile(rf"\b[A-Za-z0-9_]*(?:{family_type_alt})[A-Za-z0-9_]*\b"),
            "concrete market-family type name in core production code",
        ),
        Rule(
            "src/bolt_v3_adapters.rs",
            re.compile(
                rf"\benum\s+BoltV3VenueAdapterConfig\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_type_alt})\b",
                re.DOTALL,
            ),
            "closed provider adapter config enum",
        ),
        Rule(
            "src/bolt_v3_adapters.rs",
            re.compile(
                rf"\bmatch\s+(?:venue\.kind\.as_str\(\)|kind)\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_alt})::KEY\b",
                re.DOTALL,
            ),
            "adapter mapping dispatches on concrete provider key",
        ),
        Rule(
            "src/bolt_v3_adapters.rs",
            re.compile(r"\bMarketSlugFilter\b"),
            "provider-specific NT filter in adapter mapper",
        ),
        Rule(
            "src/bolt_v3_secrets.rs",
            re.compile(
                rf"\benum\s+ResolvedBoltV3VenueSecrets\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_type_alt})\b",
                re.DOTALL,
            ),
            "closed resolved venue secret enum",
        ),
        Rule(
            "src/bolt_v3_secrets.rs",
            re.compile(
                rf"\bmatch\s+(?:venue\.kind\.as_str\(\)|kind)\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_alt})::KEY\b",
                re.DOTALL,
            ),
            "secret resolution dispatches on concrete provider key",
        ),
        Rule(
            "src/bolt_v3_client_registration.rs",
            re.compile(rf"\buse\s+nautilus_(?:{provider_alt})::factories\b"),
            "concrete NT provider factory import",
        ),
        Rule(
            "src/bolt_v3_client_registration.rs",
            re.compile(
                rf"\benum\s+BoltV3RegisteredVenue\s*\{{(?P<body>[^}}]*)"
                rf"\b(?:{provider_type_alt})\b",
                re.DOTALL,
            ),
            "closed registered venue summary enum",
        ),
        Rule(
            "src/bolt_v3_client_registration.rs",
            re.compile(rf"\bBoltV3VenueAdapterConfig::(?:{provider_type_alt})\b"),
            "client registration dispatches on concrete adapter variant",
        ),
        *rules_for(
            provider_binding_files,
            re.compile(rf"\bbolt_v3_market_families::(?:{family_alt})::"),
            "provider binding accesses concrete market-family module path",
        ),
        *rules_for(
            provider_binding_files,
            re.compile(rf"\b[A-Za-z0-9_]*(?:{family_type_alt})[A-Za-z0-9_]*\b"),
            "concrete market-family type name in provider binding code",
        ),
        *rules_for(
            provider_binding_files,
            re.compile(rf'"(?:{family_alt})"'),
            "market-family key string literal in provider binding code",
        ),
    ]


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def excerpt_for(text: str, pos: int) -> str:
    line_start = text.rfind("\n", 0, pos) + 1
    line_end = text.find("\n", pos)
    if line_end == -1:
        line_end = len(text)
    return " ".join(text[line_start:line_end].strip().split())


def is_cfg_test_attr(stripped: str) -> bool:
    bounds = cfg_attr_bounds(stripped)
    if bounds is None:
        return False

    expression_start, expression_end, _ = bounds
    expression = stripped[expression_start:expression_end]
    can_be_true_without_test, _ = cfg_truth_without_test(expression)
    return not can_be_true_without_test


def cfg_attr_is_inner(stripped: str) -> bool:
    if not stripped.startswith("#"):
        return False
    i = 1
    while i < len(stripped) and stripped[i].isspace():
        i += 1
    return i < len(stripped) and stripped[i] == "!"


def cfg_attr_expression_start(stripped: str) -> int | None:
    if not stripped.startswith("#"):
        return None

    i = 1
    while i < len(stripped) and stripped[i].isspace():
        i += 1
    if i < len(stripped) and stripped[i] == "!":
        i += 1
        while i < len(stripped) and stripped[i].isspace():
            i += 1
    if i >= len(stripped) or stripped[i] != "[":
        return None

    i += 1
    while i < len(stripped) and stripped[i].isspace():
        i += 1
    if not stripped.startswith("cfg", i):
        return None

    i += len("cfg")
    if i < len(stripped) and (stripped[i].isalnum() or stripped[i] == "_"):
        return None
    while i < len(stripped) and stripped[i].isspace():
        i += 1
    if i >= len(stripped) or stripped[i] != "(":
        return None
    return i + 1


def cfg_attr_bounds(stripped: str) -> tuple[int, int, int] | None:
    expression_start = cfg_attr_expression_start(stripped)
    if expression_start is None:
        return None

    quote: str | None = None
    escaped = False
    depth = 1
    i = expression_start
    while i < len(stripped):
        char = stripped[i]
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            i += 1
            continue

        if char == '"':
            quote = char
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                expression_end = i
                i += 1
                while i < len(stripped) and stripped[i].isspace():
                    i += 1
                return (
                    expression_start,
                    expression_end,
                    i + 1,
                ) if i < len(stripped) and stripped[i] == "]" else None
        i += 1

    return None


def cfg_attr_end_index(stripped: str) -> int | None:
    bounds = cfg_attr_bounds(stripped)
    return bounds[2] if bounds is not None else None


def split_top_level_args(expression: str) -> list[str]:
    parts: list[str] = []
    start = 0
    depth = 0
    quote: str | None = None
    escaped = False

    for i, char in enumerate(expression):
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            continue

        if char == '"':
            quote = char
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        elif char == "," and depth == 0:
            parts.append(expression[start:i].strip())
            start = i + 1

    parts.append(expression[start:].strip())
    return [part for part in parts if part]


def cfg_call_inner(expression: str, name: str) -> str | None:
    match = re.match(rf"{name}\s*\(", expression)
    if match is None:
        return None

    open_paren = match.end() - 1
    depth = 0
    quote: str | None = None
    escaped = False
    for i in range(open_paren, len(expression)):
        char = expression[i]
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            continue

        if char == '"':
            quote = char
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                if expression[i + 1 :].strip():
                    return None
                return expression[open_paren + 1 : i]

    return None


def cfg_truth_without_test(expression: str) -> tuple[bool, bool]:
    """Return whether cfg expression can be true/false when cfg(test) is false.

    Unknown predicates (features, targets, custom cfgs) are treated as either
    true or false. A cfg item is test-only only when it cannot be true in any
    non-test configuration.
    """

    expression = expression.strip()
    if not expression:
        return True, True

    top_level_args = split_top_level_args(expression)
    if len(top_level_args) > 1:
        return cfg_all_truth_without_test(top_level_args)

    inner = cfg_call_inner(expression, "all")
    if inner is not None:
        return cfg_all_truth_without_test(split_top_level_args(inner))

    inner = cfg_call_inner(expression, "any")
    if inner is not None:
        args = split_top_level_args(inner)
        if not args:
            return True, True
        truths = [cfg_truth_without_test(arg) for arg in args]
        can_be_true = any(can_true for can_true, _ in truths)
        can_be_false = all(can_false for _, can_false in truths)
        return can_be_true, can_be_false

    inner = cfg_call_inner(expression, "not")
    if inner is not None:
        args = split_top_level_args(inner)
        if len(args) != 1:
            return True, True
        can_be_true, can_be_false = cfg_truth_without_test(args[0])
        return can_be_false, can_be_true

    if expression == "test":
        return False, True

    return True, True


def cfg_all_truth_without_test(args: list[str]) -> tuple[bool, bool]:
    if not args:
        return True, True

    truths = [cfg_truth_without_test(arg) for arg in args]
    can_be_true = all(can_true for can_true, _ in truths)
    can_be_false = any(can_false for _, can_false in truths)
    return can_be_true, can_be_false


def raw_string_closer_at(text: str, start: int) -> tuple[int, str] | None:
    prefix_start = start
    if text.startswith("br", start):
        start += 2
    elif text.startswith("r", start):
        start += 1
    else:
        return None

    hash_start = start
    while start < len(text) and text[start] == "#":
        start += 1
    if start >= len(text) or text[start] != '"':
        return None

    hashes = text[hash_start:start]
    return start + 1 - prefix_start, '"' + hashes


def char_literal_end_at(line: str, start: int) -> int | None:
    if start >= len(line) or line[start] != "'":
        return None

    i = start + 1
    if i >= len(line) or line[i] in ("'", "\n"):
        return None

    if line[i] != "\\":
        quote = i + 1
        return quote + 1 if quote < len(line) and line[quote] == "'" else None

    if i + 1 >= len(line):
        return None

    escape = line[i + 1]
    if escape == "x":
        hex_digits = line[i + 2 : i + 4]
        quote = i + 4
        if len(hex_digits) == 2 and all(char in string.hexdigits for char in hex_digits):
            return quote + 1 if quote < len(line) and line[quote] == "'" else None
        return None

    if escape == "u" and i + 2 < len(line) and line[i + 2] == "{":
        close_brace = line.find("}", i + 3)
        if close_brace == -1:
            return None
        hex_digits = line[i + 3 : close_brace]
        quote = close_brace + 1
        if 1 <= len(hex_digits) <= 6 and all(char in string.hexdigits for char in hex_digits):
            return quote + 1 if quote < len(line) and line[quote] == "'" else None
        return None

    quote = i + 2
    return quote + 1 if quote < len(line) and line[quote] == "'" else None


def strip_comments_preserve_lines(text: str) -> str:
    output: list[str] = []
    i = 0
    block_depth = 0
    string_quote: str | None = None
    raw_string_closer: str | None = None
    escaped = False

    while i < len(text):
        char = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""

        if block_depth:
            if char == "/" and nxt == "*":
                block_depth += 1
                i += 2
                continue
            if char == "*" and nxt == "/":
                block_depth -= 1
                i += 2
                continue
            output.append("\n" if char == "\n" else " ")
            i += 1
            continue

        if raw_string_closer is not None:
            if text.startswith(raw_string_closer, i):
                output.append(text[i : i + len(raw_string_closer)])
                i += len(raw_string_closer)
                raw_string_closer = None
            else:
                output.append(char)
                i += 1
            continue

        if string_quote is not None:
            output.append(char)
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == string_quote:
                string_quote = None
            i += 1
            continue

        if char == "'":
            char_end = char_literal_end_at(text, i)
            if char_end is not None:
                output.append(text[i:char_end])
                i = char_end
                continue

        if char == "/" and nxt == "/":
            while i < len(text) and text[i] != "\n":
                output.append(" ")
                i += 1
            continue

        if char == "/" and nxt == "*":
            block_depth = 1
            output.extend((" ", " "))
            i += 2
            continue

        if char == '"':
            string_quote = char
        else:
            raw = raw_string_closer_at(text, i)
            if raw is not None:
                prefix_len, raw_string_closer = raw
                output.append(text[i : i + prefix_len])
                i += prefix_len
                continue

        output.append(char)
        i += 1

    return "".join(output)


@dataclass
class BraceScanState:
    quote: str | None = None
    raw_string_closer: str | None = None
    escaped: bool = False


@dataclass
class ItemScanState:
    brace: BraceScanState = field(default_factory=BraceScanState)
    brace_depth: int = 0
    paren_depth: int = 0
    bracket_depth: int = 0
    saw_brace: bool = False
    saw_where: bool = False


def brace_delta(line: str, state: BraceScanState | None = None) -> int:
    if state is None:
        state = BraceScanState()

    delta = 0
    i = 0

    while i < len(line):
        char = line[i]
        if state.raw_string_closer is not None:
            if line.startswith(state.raw_string_closer, i):
                i += len(state.raw_string_closer)
                state.raw_string_closer = None
            else:
                i += 1
            continue

        if state.quote is not None:
            if state.escaped:
                state.escaped = False
            elif char == "\\":
                state.escaped = True
            elif char == state.quote:
                state.quote = None
            i += 1
            continue

        raw = raw_string_closer_at(line, i)
        if raw is not None:
            prefix_len, state.raw_string_closer = raw
            i += prefix_len
            continue

        if char == '"':
            state.quote = char
            i += 1
            continue
        if char == "'":
            char_end = char_literal_end_at(line, i)
            if char_end is not None:
                i = char_end
                continue
        if char == "{":
            delta += 1
        elif char == "}":
            delta -= 1
        i += 1

    return delta


def cfg_item_complete(line: str, state: ItemScanState) -> bool:
    i = 0
    while i < len(line):
        char = line[i]
        if state.brace.raw_string_closer is not None:
            if line.startswith(state.brace.raw_string_closer, i):
                i += len(state.brace.raw_string_closer)
                state.brace.raw_string_closer = None
            else:
                i += 1
            continue

        if state.brace.quote is not None:
            if state.brace.escaped:
                state.brace.escaped = False
            elif char == "\\":
                state.brace.escaped = True
            elif char == state.brace.quote:
                state.brace.quote = None
            i += 1
            continue

        raw = raw_string_closer_at(line, i)
        if raw is not None:
            prefix_len, state.brace.raw_string_closer = raw
            i += prefix_len
            continue

        if char == '"':
            state.brace.quote = char
            i += 1
            continue
        if char == "'":
            char_end = char_literal_end_at(line, i)
            if char_end is not None:
                i = char_end
                continue
        if char == "{":
            state.saw_brace = True
            state.brace_depth += 1
        elif char == "}":
            state.brace_depth -= 1
            if state.brace_depth < 0:
                return True
            if state.saw_brace and state.brace_depth <= 0:
                return True
        elif char.isalpha() or char == "_":
            start = i
            while i < len(line) and (line[i].isalnum() or line[i] == "_"):
                i += 1
            if (
                state.paren_depth == 0
                and state.bracket_depth == 0
                and state.brace_depth == 0
                and line[start:i] == "where"
            ):
                state.saw_where = True
            continue
        elif char == "(":
            state.paren_depth += 1
        elif char == ")" and state.paren_depth:
            state.paren_depth -= 1
        elif char == "[":
            state.bracket_depth += 1
        elif char == "]" and state.bracket_depth:
            state.bracket_depth -= 1
        elif (
            not state.saw_brace
            and not state.saw_where
            and state.paren_depth == 0
            and state.bracket_depth == 0
            and char in (";", ",")
        ):
            return True
        i += 1

    return False


def production_text(text: str) -> str:
    """Return a scan view that excludes comments and inline test modules.

    The verifier targets production architecture leakage. Comment prose and
    `#[cfg(test)]` fixtures are useful context, but they are not the
    core surface that decides whether adding a provider requires runtime edits.
    """

    lines = strip_comments_preserve_lines(text).splitlines()
    output: list[str] = []
    production_state = BraceScanState()
    pending_cfg_item: ItemScanState | None = None

    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.lstrip()
        if pending_cfg_item is not None:
            output.append("")
            if cfg_item_complete(line, pending_cfg_item):
                pending_cfg_item = None
            i += 1
            continue

        if (
            production_state.quote is None
            and production_state.raw_string_closer is None
            and cfg_attr_expression_start(stripped) is not None
        ):
            attr_lines = [line]
            attr_text = stripped
            j = i
            bounds = cfg_attr_bounds(attr_text)
            while bounds is None and j + 1 < len(lines):
                j += 1
                attr_lines.append(lines[j])
                attr_text += "\n" + lines[j].lstrip()
                bounds = cfg_attr_bounds(attr_text)

            if bounds is not None:
                expression_start, expression_end, attr_end = bounds
                expression = attr_text[expression_start:expression_end]
                can_be_true_without_test, _ = cfg_truth_without_test(expression)
                if not can_be_true_without_test:
                    output.extend("" for _ in attr_lines)
                    if cfg_attr_is_inner(stripped):
                        output.extend("" for _ in lines[j + 1 :])
                        break

                    gated_item = attr_text[attr_end:].strip()
                    if gated_item:
                        item_state = ItemScanState()
                        if not cfg_item_complete(gated_item, item_state):
                            pending_cfg_item = item_state
                    else:
                        pending_cfg_item = ItemScanState()
                    i = j + 1
                    continue

                for attr_line in attr_lines:
                    output.append(attr_line)
                    # Maintain string/raw-string context for subsequent lines.
                    brace_delta(attr_line, production_state)
                i = j + 1
                continue

        if not stripped:
            output.append(line)
            i += 1
            continue

        output.append(line)
        # Maintain string/raw-string context for subsequent lines.
        brace_delta(line, production_state)
        i += 1

    trailing_newline = "\n" if text.endswith("\n") else ""
    return "\n".join(output) + trailing_newline


def scan_root(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for rule in rules_for_root(root):
        path = root / rule.path
        if not path.exists():
            continue
        text = production_text(path.read_text(encoding="utf-8"))
        for match in rule.pattern.finditer(text):
            finding = Finding(
                path=rule.path,
                line=line_number(text, match.start()),
                message=rule.message,
                excerpt=excerpt_for(text, match.start()),
            )
            if not is_allowed_finding(finding):
                findings.append(finding)
    return findings


def is_allowed_finding(finding: Finding) -> bool:
    return any(
        finding.path == allowance.path
        and finding.message == allowance.message
        and finding.excerpt == allowance.exact_excerpt
        for allowance in FINDING_ALLOWANCES
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=REPO_ROOT,
        help="repository root to scan; defaults to this checkout",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    findings = scan_root(args.root)

    if findings:
        for finding in findings:
            print(finding.render("FAIL"), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 provider-leak verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
