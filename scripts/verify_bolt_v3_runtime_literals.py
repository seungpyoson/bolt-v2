#!/usr/bin/env python3
"""Verify Bolt-v3 production literals are classified.

This verifier scans every current production Rust source file under `src/`,
skips `#[cfg(test)] mod tests` regions and diagnostics by rule, then requires
every remaining candidate runtime literal to be explicitly allowlisted with a
rationale.
"""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

from verify_bolt_v3_provider_leaks import (
    production_text as production_source_text,
)


REPO_ROOT = Path(__file__).resolve().parent.parent
AUDIT_PATH = REPO_ROOT / "docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml"
SCAN_GLOBS = (
    "src/**/*.rs",
)

DIAGNOSTIC_WORDS = (
    "already",
    "allowed",
    "contains",
    "declares",
    "error",
    "expected",
    "exceeded",
    "failed",
    "forbidden",
    "invalid",
    "missing",
    "must",
    "requires",
    "required",
    "rejected",
    "resolved",
    "supported",
    "unsupported",
    "unknown",
)

IGNORED_CONTEXT_PATTERNS = [
    r"^\s*#\[serde\(",
    r"\bwrite!?\(",
    r"\bwriteln!\(",
    r"\beprintln!\(",
    r"\blog::(debug|error|info|trace|warn)!\(",
    r"\bformat!\(",
    r"\bpanic!\(",
    r"\banyhow::anyhow!\(",
    r"\banyhow::bail!\(",
    r"\bexpect\(",
    r"\bdebug_struct\(",
    r"\bdebug_tuple\(",
    r"\bf\.write_str\(",
    r"\.field\(",
    r"\bPathBuf::from\(",
    r"\bok_or_else\(",
    r"\bmap_err\(",
    r"\bsource:\s*format!\(",
    r"\bfield:\s*\"",
    r"\bblock:\s*\"",
    r"\bmessage:\s*\"",
    r"\bself\.0\b",
    r"\.len\(\)\s*(==|>)\s*1\b",
]

IGNORED_CALL_CONTEXT_PATTERNS = [
    r"\blog::(debug|error|info|trace|warn)!\s*(\(|\[|\{)",
    r"\btracing::(debug|error|info|trace|warn)!\s*(\(|\[|\{)",
]

DIAGNOSTIC_MACRO_PATTERN = re.compile(
    r"\b(?:log|tracing)::(?:debug|error|info|trace|warn)!\s*(\(|\[|\{)"
)

@dataclass(frozen=True)
class Literal:
    path: str
    line: int
    kind: str
    literal: str
    context: str
    call_context: str

    def key(self) -> tuple[str, str, str, str]:
        return (self.path, self.kind, self.literal, self.context)


def load_allowed() -> set[tuple[str, str, str, str]]:
    audit = tomllib.loads(AUDIT_PATH.read_text(encoding="utf-8"))
    allowed: set[tuple[str, str, str, str]] = set()
    for row in audit.get("allowed", []):
        missing = [
            name
            for name in ("path", "kind", "literal", "context", "classification", "reason")
            if not row.get(name)
        ]
        if missing:
            joined = ", ".join(missing)
            raise ValueError(f"{AUDIT_PATH}: allowlist row missing {joined}: {row!r}")
        if row["classification"] == "provider_credential_log_module" and (
            not row["path"].startswith("src/bolt_v3_providers/")
            or row["path"] == "src/bolt_v3_providers/mod.rs"
        ):
            raise ValueError(
                f"{AUDIT_PATH}: provider_credential_log_module must be owned by "
                f"a concrete provider module: {row!r}"
            )
        allowed.add((row["path"], row["kind"], row["literal"], row["context"]))
    return allowed


def rust_char_literal_end(text: str, start: int) -> int | None:
    """Return the end offset for a Rust char literal starting at `start`.

    Lifetimes such as `'a` are intentionally not matched because they do not
    close with a second quote.
    """

    if start >= len(text) or text[start] != "'":
        return None

    index = start + 1
    if index >= len(text) or text[index] in {"\n", "\r", "'"}:
        return None

    if text[index] == "\\":
        index += 1
        if index >= len(text):
            return None
        if text[index] == "u" and index + 1 < len(text) and text[index + 1] == "{":
            index += 2
            while index < len(text) and text[index] != "}":
                if text[index] in {"\n", "\r"}:
                    return None
                index += 1
            if index >= len(text):
                return None
            index += 1
        elif (
            text[index] == "x"
            and index + 2 < len(text)
            and all(char in "0123456789abcdefABCDEF" for char in text[index + 1 : index + 3])
        ):
            index += 3
        else:
            index += 1
    else:
        index += 1

    if index < len(text) and text[index] == "'":
        return index + 1
    return None


def rust_raw_string_literal_end(text: str, start: int) -> int | None:
    """Return the end offset for a Rust raw string or byte string literal."""

    raw_match = re.match(r'br(#+)?"|r(#+)?"', text[start:])
    if not raw_match:
        return None
    hashes = raw_match.group(1) or raw_match.group(2) or ""
    terminator = '"' + hashes
    index = start + len(raw_match.group(0))
    end = text.find(terminator, index)
    return len(text) if end == -1 else end + len(terminator)


def rust_string_literal_end(text: str, start: int) -> int | None:
    """Return the end offset for a Rust string or byte string literal."""

    if start < len(text) and text[start] == '"':
        index = start + 1
    elif start + 1 < len(text) and text[start] == "b" and text[start + 1] == '"':
        index = start + 2
    else:
        return None

    escaped = False
    while index < len(text):
        current = text[index]
        if escaped:
            escaped = False
        elif current == "\\":
            escaped = True
        elif current == '"':
            return index + 1
        index += 1
    return len(text)


def line_context(text: str, line: int) -> str:
    return text.splitlines()[line - 1].strip()


def call_context(text: str, literal_start: int) -> str:
    for match in reversed(list(DIAGNOSTIC_MACRO_PATTERN.finditer(text, 0, literal_start))):
        opener_index = match.end(1) - 1
        if delimiter_is_open_at(text, opener_index, literal_start):
            line_start = text.rfind("\n", 0, match.start()) + 1
            return text[line_start:literal_start].strip()
    return ""


def delimiter_is_open_at(text: str, opener_index: int, stop: int) -> bool:
    pairs = {"(": ")", "[": "]", "{": "}"}
    openers = set(pairs)
    closers = {value: key for key, value in pairs.items()}
    stack: list[str] = []
    index = opener_index
    block_comment_depth = 0

    while index < stop:
        char = text[index]
        nxt = text[index + 1] if index + 1 < stop else ""

        if block_comment_depth:
            if char == "/" and nxt == "*":
                block_comment_depth += 1
                index += 2
                continue
            if char == "*" and nxt == "/":
                block_comment_depth -= 1
                index += 2
                continue
            index += 1
            continue

        if char == "/" and nxt == "/":
            newline = text.find("\n", index, stop)
            if newline == -1:
                return bool(stack)
            index = newline + 1
            continue

        if char == "/" and nxt == "*":
            block_comment_depth = 1
            index += 2
            continue

        raw_end = rust_raw_string_literal_end(text, index)
        if raw_end is not None:
            index = min(raw_end, stop)
            continue

        if char == "b" and nxt == "'":
            char_end = rust_char_literal_end(text, index + 1)
            if char_end is not None:
                index = min(char_end, stop)
                continue
        char_end = rust_char_literal_end(text, index)
        if char_end is not None:
            index = min(char_end, stop)
            continue

        string_end = rust_string_literal_end(text, index)
        if string_end is not None:
            index = min(string_end, stop)
            continue

        if char in openers:
            stack.append(char)
        elif char in closers:
            if not stack or stack[-1] != closers[char]:
                return False
            stack.pop()
            if not stack:
                return False
        index += 1

    return bool(stack)


def scan_file(path: Path) -> list[Literal]:
    text = production_source_text(path.read_text(encoding="utf-8"))
    rel = str(path.relative_to(REPO_ROOT))
    literals: list[Literal] = []
    index = 0
    line = 1
    block_comment_depth = 0

    def current_context() -> str:
        return line_context(text, line)

    while index < len(text):
        char = text[index]
        nxt = text[index + 1] if index + 1 < len(text) else ""

        if block_comment_depth:
            if char == "/" and nxt == "*":
                block_comment_depth += 1
                index += 2
                continue
            if char == "*" and nxt == "/":
                block_comment_depth -= 1
                index += 2
                continue
            if char == "\n":
                line += 1
            index += 1
            continue

        if char == "/" and nxt == "/":
            newline = text.find("\n", index)
            if newline == -1:
                break
            index = newline + 1
            line += 1
            continue

        if char == "/" and nxt == "*":
            block_comment_depth = 1
            index += 2
            continue

        raw_end = rust_raw_string_literal_end(text, index)
        if raw_end is not None:
            start = index
            literal = text[start:raw_end]
            literals.append(
                Literal(rel, line, "string", literal, current_context(), call_context(text, start))
            )
            line += literal.count("\n")
            index = raw_end
            continue

        if char == "b" and nxt == "'":
            char_end = rust_char_literal_end(text, index + 1)
            if char_end is not None:
                index = char_end
                continue
        char_end = rust_char_literal_end(text, index)
        if char_end is not None:
            index = char_end
            continue

        string_end = rust_string_literal_end(text, index)
        if string_end is not None:
            start = index
            literal = text[start:string_end]
            literal_line = line
            literals.append(
                Literal(
                    rel,
                    literal_line,
                    "string",
                    literal,
                    current_context(),
                    call_context(text, start),
                )
            )
            line += literal.count("\n")
            index = string_end
            continue

        number_end = rust_number_literal_end(text, index)
        if number_end is not None:
            start = index
            index = number_end
            literals.append(
                Literal(
                    rel,
                    line,
                    "number",
                    text[start:index],
                    current_context(),
                    call_context(text, start),
                )
            )
            continue

        if char == "\n":
            line += 1
        index += 1

    return literals


def is_ident_char(char: str) -> bool:
    return char.isalnum() or char == "_"


def rust_number_literal_end(text: str, start: int) -> int | None:
    if start >= len(text):
        return None
    literal_start = start
    if text[start] == "-":
        if start + 1 >= len(text) or not text[start + 1].isdigit():
            return None
        if not is_unary_minus_context(text, start):
            return None
        start += 1
    elif not text[start].isdigit():
        return None
    elif is_ident_char(text[start - 1] if start > 0 else ""):
        return None

    if literal_start != start and is_ident_char(
        text[literal_start - 1] if literal_start > 0 else ""
    ):
        return None

    index = start
    if text[index] == "0" and index + 1 < len(text) and text[index + 1] in {"b", "o", "x"}:
        index += 2
        while index < len(text) and (text[index].isalnum() or text[index] == "_"):
            index += 1
        if is_ident_char(text[index] if index < len(text) else ""):
            return None
        return index

    while index < len(text) and (text[index].isdigit() or text[index] == "_"):
        index += 1
    if (
        index + 1 < len(text)
        and text[index] == "."
        and text[index + 1] != "."
        and text[index + 1].isdigit()
    ):
        index += 1
        while index < len(text) and (text[index].isdigit() or text[index] == "_"):
            index += 1
    if index < len(text) and text[index] in {"e", "E"}:
        exponent = index + 1
        if exponent < len(text) and text[exponent] in {"+", "-"}:
            exponent += 1
        if exponent < len(text) and text[exponent].isdigit():
            index = exponent + 1
            while index < len(text) and (text[index].isdigit() or text[index] == "_"):
                index += 1
    if index + 1 < len(text) and text[index] == "_" and text[index + 1].isalpha():
        index += 1
    if index < len(text) and text[index].isalpha():
        index += 1
        while index < len(text) and is_ident_char(text[index]):
            index += 1
    if is_ident_char(text[index] if index < len(text) else ""):
        return None
    return index


def is_unary_minus_context(text: str, start: int) -> bool:
    index = start - 1
    while index >= 0 and text[index].isspace():
        index -= 1
    if index < 0:
        return True

    char = text[index]
    if char in "([{,:;=,!<>+-*/%&|^?":
        return True

    if is_ident_char(char):
        end = index + 1
        while index >= 0 and is_ident_char(text[index]):
            index -= 1
        return text[index + 1 : end] in {"return", "break", "continue"}

    return False


def is_ignored_by_rule(literal: Literal) -> bool:
    context = literal.context
    if any(re.search(pattern, context) for pattern in IGNORED_CONTEXT_PATTERNS):
        return True
    if any(re.search(pattern, literal.call_context) for pattern in IGNORED_CALL_CONTEXT_PATTERNS):
        return True
    if literal.kind == "string":
        text = literal.literal.strip('"')
        if text.startswith(("nautilus.", "risk.")) or text.endswith("_ssm_path"):
            return True
        if "SUBMIT_ADMISSION_STATUS_REJECTED" in context:
            return False
        lowered = text.lower()
        words = set(re.findall(r"[a-z_]+", lowered))
        if words & set(DIAGNOSTIC_WORDS):
            return True
        if " not " in f" {lowered} ":
            return True
        if text in {"", "s", "hours", "minutes", "seconds"}:
            return True
    if literal.kind == "number":
        if literal.literal in {"0", "1"} and re.search(r"\b(if|match)\b", context):
            return True
    return False


def scan_literals() -> list[Literal]:
    candidates: list[Literal] = []
    paths = {
        path
        for pattern in SCAN_GLOBS
        for path in REPO_ROOT.glob(pattern)
        if path.is_file()
    }
    for path in sorted(paths):
        for literal in scan_file(path):
            if not is_ignored_by_rule(literal):
                candidates.append(literal)
    return candidates


def main() -> int:
    try:
        allowed = load_allowed()
    except Exception as error:
        print(f"ERROR: failed to load runtime literal audit: {error}", file=sys.stderr)
        return 2

    scanned_literals = scan_literals()
    unclassified = [literal for literal in scanned_literals if literal.key() not in allowed]
    stale = allowed - {literal.key() for literal in scanned_literals}

    if unclassified or stale:
        for literal in unclassified:
            print(
                "FAIL: unclassified Bolt-v3 production literal "
                f"{literal.path}:{literal.line}: {literal.kind} {literal.literal} "
                f"in `{literal.context}`",
                file=sys.stderr,
            )
        for path, kind, literal, context in sorted(stale):
            print(
                "FAIL: stale Bolt-v3 runtime literal allowlist entry "
                f"{path}: {kind} {literal} in `{context}`",
                file=sys.stderr,
            )
        return 1

    print("OK: Bolt-v3 runtime literal audit passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
