#!/usr/bin/env python3
"""Verify Bolt-v3 production literals are classified.

This verifier is intentionally narrow. It scans production code in root
`src/bolt_v3_*.rs` files plus files under `src/bolt_v3_*` module directories,
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


REPO_ROOT = Path(__file__).resolve().parent.parent
AUDIT_PATH = REPO_ROOT / "docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml"
SCAN_GLOBS = ("src/bolt_v3_*.rs", "src/bolt_v3_*/**/*.rs")

DIAGNOSTIC_WORDS = (
    "already",
    "contains",
    "does not",
    "expected",
    "failed",
    "forbidden",
    "invalid",
    "missing",
    "must",
    "rejected",
    "resolved",
    "unsupported",
)

IGNORED_CONTEXT_PATTERNS = [
    r"^\s*#\[serde\(",
    r"\bwrite!?\(",
    r"\bwriteln!\(",
    r"\bformat!\(",
    r"\bpanic!\(",
    r"\bexpect\(",
    r"\bdebug_struct\(",
    r"\bdebug_tuple\(",
    r"\bf\.write_str\(",
    r"\.field\(",
    r"\.contains\(",
    r"\.join\(",
    r"\.as_str\(",
    r'^\s*\("[a-z0-9_]+",\s*',
    r'^\s*"[a-z0-9_]+",\s*$',
    r"\bPathBuf::from\(",
    r"\bErr\(",
    r"\bok_or_else\(",
    r"\bmap_err\(",
    r"\bsource:\s*format!\(",
    r"\bfield:\s*\"",
    r"\bblock:\s*\"",
    r"\bmessage:\s*\"",
    r"\bself\.0\b",
    r"\.len\(\)\s*(==|>)\s*1\b",
    r"\bif\b.*[=!]=\s*0\b",
    r"\bif\b.*>=\s*60\b",
    r"\bif\b.*&&.*==\s*0\b",
]


@dataclass(frozen=True)
class Literal:
    path: str
    line: int
    kind: str
    literal: str
    context: str

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
        allowed.add((row["path"], row["kind"], row["literal"], row["context"]))
    return allowed


def test_ranges(text: str) -> list[tuple[int, int]]:
    """Return byte ranges for file-local `#[cfg(test)] mod tests` blocks.

    This scanner only needs enough Rust awareness to ignore braces inside
    comments and string/char literals while finding the end of the test module.
    """

    lines = text.splitlines(keepends=True)
    starts = []
    offset = 0
    for line in lines:
        starts.append(offset)
        offset += len(line)

    ranges: list[tuple[int, int]] = []
    pending_cfg = False
    for index, line in enumerate(lines):
        cfg_match = re.search(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", line)
        mod_match = re.search(r"(?:pub\s+)?mod\s+tests\s*\{", line)
        if mod_match and (pending_cfg or cfg_match):
            start = starts[index]
            opening = text.find("{", start + mod_match.start())
            closing = matching_brace(text, opening)
            ranges.append((start, closing + 1 if closing is not None else len(text)))
            pending_cfg = False
            continue
        if cfg_match:
            pending_cfg = line[cfg_match.end() :].strip() == ""
            continue
        if pending_cfg:
            stripped = line.strip()
            if stripped == "" or stripped.startswith("//") or stripped.startswith("#["):
                continue
            pending_cfg = False
    return ranges


def matching_brace(text: str, opening: int) -> int | None:
    depth = 0
    index = opening
    block_comment_depth = 0

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
            index += 1
            continue

        if char == "/" and nxt == "/":
            newline = text.find("\n", index)
            if newline == -1:
                return None
            index = newline + 1
            continue
        if char == "/" and nxt == "*":
            block_comment_depth = 1
            index += 2
            continue
        raw_end = rust_raw_string_literal_end(text, index)
        if raw_end is not None:
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
            index = string_end
            continue
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index
        index += 1

    return None


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


def inside_ranges(position: int, ranges: list[tuple[int, int]]) -> bool:
    return any(start <= position < end for start, end in ranges)


def line_context(text: str, line: int) -> str:
    return text.splitlines()[line - 1].strip()


def scan_file(path: Path) -> list[Literal]:
    text = path.read_text(encoding="utf-8")
    rel = str(path.relative_to(REPO_ROOT))
    ranges = test_ranges(text)
    literals: list[Literal] = []
    index = 0
    line = 1
    block_comment_depth = 0

    def current_context() -> str:
        return line_context(text, line)

    while index < len(text):
        char = text[index]
        nxt = text[index + 1] if index + 1 < len(text) else ""

        if inside_ranges(index, ranges):
            if char == "\n":
                line += 1
            index += 1
            continue

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
            literals.append(Literal(rel, line, "string", literal, current_context()))
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
            literals.append(Literal(rel, line - literal.count("\n"), "string", literal, current_context()))
            line += literal.count("\n")
            index = string_end
            continue

        if char.isdigit() and not is_ident_char(text[index - 1] if index > 0 else ""):
            start = index
            index += 1
            while index < len(text) and (text[index].isdigit() or text[index] in "_."):
                index += 1
            if not is_ident_char(text[index] if index < len(text) else ""):
                literals.append(Literal(rel, line, "number", text[start:index], current_context()))
                continue

        if char == "\n":
            line += 1
        index += 1

    return literals


def is_ident_char(char: str) -> bool:
    return char.isalnum() or char == "_"


def is_ignored_by_rule(literal: Literal) -> bool:
    context = literal.context
    if any(re.search(pattern, context) for pattern in IGNORED_CONTEXT_PATTERNS):
        return True
    if literal.kind == "string":
        text = literal.literal.strip('"')
        if text.startswith(("nautilus.", "risk.")) or text.endswith("_ssm_path"):
            return True
        if "{" in text or "}" in text:
            return True
        lowered = text.lower()
        if any(word in lowered for word in DIAGNOSTIC_WORDS):
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
