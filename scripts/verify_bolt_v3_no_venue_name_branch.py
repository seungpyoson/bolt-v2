#!/usr/bin/env python3
"""FR-080: forbid venue-name string-literal branches outside provider modules.

The capability contract (D8/FR-080) requires the controller to branch on venue
*capabilities* read from `VenueContract`, never on a hardcoded venue name. This
fence catches venue-name string-literal BRANCHES — equality (`==`/`!=`), the
string membership/compare methods, `matches!`, `if/while let`, and `match` arms
whose scrutinee is a venue-identity read — in `src/**/*.rs` outside
`src/bolt_v3_providers/` (where venue-name KEY literals legitimately live).

DESIGN — soundness over completeness. A *text* fence cannot decide arbitrary
Rust semantics, so it is built to NEVER flag valid code (no false positives)
while covering the idiomatic evasions:

  * Every venue-string literal *spelling* is normalized by `_canonical_code`
    before any rule runs — plain `"x"`, raw `r"x"`, raw-hashed `r#"x"#`, byte
    `b"x"`, byte-raw `br#"x"#` all collapse to a plain `"x"`, so the `r`/`b`/`#`
    prefix cannot slip a venue literal past the equality/method/match rules.
  * Comments, char literals, and *non-venue* string-literal bodies are blanked
    (newlines preserved), so a venue-branch phrase living inside a raw-string
    body or a comment is NOT scanned as code (no false positive).
  * `match` arms are flagged only when (a) the match scrutinee is itself a venue
    read AND (b) the venue literal sits in arm-*pattern* position at the match's
    top level — never in an arm body, an `if`-guard expression, or a nested
    `match` on a non-venue scrutinee. So `match mode { "gamma" => .. }` and
    `match venue { _ => { match mode { "gamma" => .. } } }` are both clean.
  * String-literal-only `concat!`, string-literal-only `format!`, and
    `VenueId::from("x")` are folded before rule matching, so those syntax forms
    cannot hide a venue token. Numeric macro literals remain GAP-4b residual.

KNOWN, DELIBERATELY-UNCAUGHT forms (they need type/flow analysis a regex cannot
do soundly; the capability-contract design + code review cover them):
  * numeric `concat!`/`format!` literals (GAP-4b), e.g. `format!("{}", true)`;
  * wrapper/alias PartialEq where neither operand is a syntactic venue read:
    `VenueId::from("polymarket") == other`, `some_non_venue_var == "polymarket"`;
  * venue-name read via a `venue`-*suffixed* getter (`obj.get_venue()`) rather
    than a `venue`-prefixed token;
  * a `match`/`if let` guard comparing a non-venue operand to a venue literal;
  * 2+ level nested-generic turbofish (`::<Vec<Cow<str>>>`).
These are accepted false negatives, recorded here so the boundary is explicit.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

from bolt_v3_source_roots import REPO_ROOT
from verify_bolt_v3_pure_rust_runtime import production_text

PROVIDERS_PREFIX = "src/bolt_v3_providers/"

_VENUES = (
    "polymarket",
    "binance",
    "bybit",
    "okx",
    "hyperliquid",
    "deribit",
    "chainlink",
    "gamma",
)
_VENUE = r"(?:" + "|".join(_VENUES) + r")"

# A venue-identity READ expression: an optional dotted receiver path, a
# `venue`-prefixed token (so `venue_id`, `venue_name`, `venue_wrapper` all
# qualify), then any zero-arg reader-accessor calls incl. turbofish
# (`.as_str()`, `.as_ref::<str>()`, `.cast::<Cow<str>>()`). The leading boundary
# stops `venue` from matching inside a larger identifier (`subvenue`,
# `revenue`, `myvenue`). Receiver/accessor repetition is bounded so a
# pathological line cannot drive quadratic backtracking.
_TURBOFISH = r"(?:\s*::\s*<[^<>]*(?:<[^<>]*>[^<>]*)*>)?"
_NAME = (
    r"(?<![A-Za-z0-9_])"
    r"(?:[A-Za-z_][A-Za-z0-9_]*\s*\.\s*){0,16}"
    r"venue[A-Za-z0-9_]*"
    + _TURBOFISH
    + r"(?:\s*\(\s*\))?"
    r"(?:\s*\.\s*[A-Za-z_][A-Za-z0-9_]*" + _TURBOFISH + r"\s*\(\s*\)){0,16}"
)
# After `_canonical_code` every venue string literal is a plain "venue".
_LIT = r'"' + _VENUE + r'"'
_EQ = r"(?:==|!=)"


@dataclass(frozen=True)
class Rule:
    label: str
    pattern: re.Pattern[str]


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    label: str
    excerpt: str


FORBIDDEN_RULES = (
    Rule("venue-name equality (name eq lit)", re.compile(rf"{_NAME}\s*{_EQ}\s*{_LIT}", re.IGNORECASE)),
    Rule("venue-name equality (lit eq name)", re.compile(rf"{_LIT}\s*{_EQ}\s*{_NAME}", re.IGNORECASE)),
    Rule(
        "venue-name membership/method",
        re.compile(
            rf"{_NAME}\s*\.\s*(?:contains|starts_with|ends_with|eq|eq_ignore_ascii_case)\s*\(\s*{_LIT}",
            re.IGNORECASE,
        ),
    ),
    Rule("venue-name matches! arm", re.compile(rf"matches!\s*\(\s*{_NAME}\s*,[^)]*{_LIT}", re.IGNORECASE)),
    Rule(
        "venue-name if/while-let",
        re.compile(rf"\b(?:if|while)\s+let\s+[^=\n]*?{_LIT}[^=\n]*?=\s*{_NAME}", re.IGNORECASE),
    ),
)

_MATCH_KW = re.compile(r"\bmatch\b")
_NAME_RE = re.compile(_NAME, re.IGNORECASE)
_LIT_RE = re.compile(_LIT, re.IGNORECASE)
_GUARD = re.compile(r"\bif\b")
_MATCH_ARM_LABEL = "venue-name match arm (venue scrutinee)"

# A complete Rust char literal: `'a'`, `'\n'`, `'\''`, `'\x41'`, `'\u{1F600}'`,
# `'"'`. Deliberately does NOT match a lifetime (`'a` with no closing quote).
_CHAR = re.compile(r"'(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]+\}|.)|[^\\'\n])'")
_IDENT = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_")


def _scan_plain(text: str, quote: int, start: int) -> tuple[int, int, str | None]:
    """Scan a plain/byte double-quoted string. `quote` = opening `"`, `start` =
    literal start (may precede `quote` for a `b"` prefix). Returns
    (start, end_exclusive, value) — value is None if the literal contains an
    escape or a newline (so it can never be a bare venue name)."""
    n = len(text)
    j = quote + 1
    escaped = False
    while j < n:
        ch = text[j]
        if ch == "\\":
            escaped = True
            j += 2
            continue
        if ch == '"':
            body = text[quote + 1 : j]
            value = None if escaped else body
            return (start, j + 1, value)
        if ch == "\n":
            return (start, j, None)  # unterminated at EOL
        j += 1
    return (start, n, None)  # unterminated at EOF


def _scan_string(text: str, i: int) -> tuple[int, int, str | None] | None:
    """If a string literal begins at index `i`, return (start, end_exclusive,
    value); else None. Handles plain, raw `r"..."`, raw-hashed `r#"..."#`, byte
    `b"..."`, and byte-raw `br#"..."#`. `value` is the decoded content, or None
    when it cannot be a bare venue name (escapes, newlines, unterminated)."""
    n = len(text)
    c = text[i]
    if c == '"':
        return _scan_plain(text, i, i)
    # raw/byte prefixes are only string starts when not the tail of an identifier
    if c in ("r", "b") and (i == 0 or text[i - 1] not in _IDENT):
        j = i
        is_byte = False
        if text[j] == "b":
            is_byte = True
            j += 1
        if j < n and text[j] == "r":
            j += 1
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                closing = '"' + ("#" * hashes)
                k = text.find(closing, j + 1)
                if k == -1:
                    return (i, n, None)
                body = text[j + 1 : k]
                value = None if "\n" in body else body
                return (i, k + len(closing), value)
            return None  # `r`/`br` not followed by a raw string body
        if is_byte and j < n and text[j] == '"':
            return _scan_plain(text, j, i)  # byte string b"..."
        return None
    return None


def _skip_ws(text: str, i: int) -> int:
    while i < len(text) and text[i].isspace():
        i += 1
    return i


def _consume_token(text: str, i: int, token: str) -> int | None:
    i = _skip_ws(text, i)
    if text.startswith(token, i):
        return i + len(token)
    return None


def _matching_paren(text: str, open_pos: int) -> int | None:
    depth = 1
    i = open_pos + 1
    while i < len(text):
        lit = _scan_string(text, i)
        if lit is not None:
            _, end, _ = lit
            i = end
            continue
        if text[i] == "'":
            match = _CHAR.match(text, i)
            if match is not None:
                i = match.end()
                continue
        if text[i] == "(":
            depth += 1
        elif text[i] == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


def _split_top_level_args(text: str) -> list[str]:
    args: list[str] = []
    start = 0
    depth = 0
    i = 0
    while i < len(text):
        lit = _scan_string(text, i)
        if lit is not None:
            _, end, _ = lit
            i = end
            continue
        if text[i] == "'":
            match = _CHAR.match(text, i)
            if match is not None:
                i = match.end()
                continue
        if text[i] in "([{":
            depth += 1
        elif text[i] in ")]}":
            depth -= 1
        elif text[i] == "," and depth == 0:
            args.append(text[start:i].strip())
            start = i + 1
        i += 1
    tail = text[start:].strip()
    if tail:
        args.append(tail)
    return args


def _literal_arg(arg: str) -> str | None:
    lit = _scan_string(arg, 0)
    if lit is None:
        return None
    _start, end, value = lit
    if value is None or arg[end:].strip():
        return None
    return value


def _format_literal_value(template: str, values: list[str]) -> str | None:
    output: list[str] = []
    value_index = 0
    i = 0
    while i < len(template):
        if template.startswith("{{", i):
            output.append("{")
            i += 2
            continue
        if template.startswith("}}", i):
            output.append("}")
            i += 2
            continue
        if template.startswith("{}", i):
            if value_index >= len(values):
                return None
            output.append(values[value_index])
            value_index += 1
            i += 2
            continue
        if template[i] in "{}":
            return None
        output.append(template[i])
        i += 1
    if value_index != len(values):
        return None
    return "".join(output)


def _folded_string_expr_at(text: str, i: int) -> tuple[int, int, str] | None:
    if text.startswith("concat!", i):
        open_pos = _skip_ws(text, i + len("concat!"))
        if open_pos >= len(text) or text[open_pos] != "(":
            return None
        close_pos = _matching_paren(text, open_pos)
        if close_pos is None:
            return None
        parts = [_literal_arg(arg) for arg in _split_top_level_args(text[open_pos + 1 : close_pos])]
        if not parts or any(part is None for part in parts):
            return None
        return i, close_pos + 1, "".join(part for part in parts if part is not None)

    if text.startswith("format!", i):
        open_pos = _skip_ws(text, i + len("format!"))
        if open_pos >= len(text) or text[open_pos] != "(":
            return None
        close_pos = _matching_paren(text, open_pos)
        if close_pos is None:
            return None
        args = _split_top_level_args(text[open_pos + 1 : close_pos])
        if not args:
            return None
        template = _literal_arg(args[0])
        if template is None:
            return None
        values = [_literal_arg(arg) for arg in args[1:]]
        if any(value is None for value in values):
            return None
        folded = _format_literal_value(template, [value for value in values if value is not None])
        if folded is None:
            return None
        return i, close_pos + 1, folded

    if not text.startswith("VenueId", i):
        return None
    cursor = _consume_token(text, i + len("VenueId"), "::")
    if cursor is None:
        return None
    cursor = _consume_token(text, cursor, "from")
    if cursor is None:
        return None
    open_pos = _skip_ws(text, cursor)
    if open_pos >= len(text) or text[open_pos] != "(":
        return None
    close_pos = _matching_paren(text, open_pos)
    if close_pos is None:
        return None
    args = _split_top_level_args(text[open_pos + 1 : close_pos])
    if len(args) != 1:
        return None
    value = _literal_arg(args[0])
    if value is None:
        return None
    return i, close_pos + 1, value


def _canonical_code(text: str) -> str:
    """Length-preserving 'code view': blank comments / char literals / non-venue
    string-literal bodies (newlines kept), and normalize any string literal
    whose value is exactly a venue name to a plain `"venue"` token so the rules
    can match it regardless of r/b/# spelling. Length is preserved so caller
    line/excerpt math against the original text stays aligned."""
    out = list(text)
    n = len(text)
    i = 0

    def blank(start: int, end: int) -> None:
        for k in range(start, end):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            j = n if j == -1 else j
            blank(i, j)
            i = j
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            j = text.find("*/", i + 2)
            j = n if j == -1 else j + 2
            blank(i, j)
            i = j
            continue
        if c == "'":
            m = _CHAR.match(text, i)
            if m:
                blank(m.start(), m.end())
                i = m.end()
                continue
            i += 1  # a lifetime (`'a`) — leave as code
            continue
        folded = _folded_string_expr_at(text, i)
        if folded is not None:
            start, end, value = folded
            blank(start, end)
            if value.lower() in _VENUES:
                token = '"' + value.lower() + '"'
                for k, ch in enumerate(token):
                    out[start + k] = ch
            i = end
            continue
        lit = _scan_string(text, i)
        if lit is not None:
            start, end, value = lit
            blank(start, end)
            if value is not None and value.lower() in _VENUES:
                token = '"' + value.lower() + '"'
                for k, ch in enumerate(token):
                    out[start + k] = ch
            i = end
            continue
        i += 1

    return "".join(out)


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def _match_arm_positions(scan_text: str) -> list[int]:
    """Positions of venue literals in the *arm-pattern* position of a `match`
    whose scrutinee is a venue read. Only depth-0 arm patterns (not arm bodies,
    not nested matches, not `if`-guard expressions) are scanned, so a benign
    nested `match mode { "gamma" => .. }` inside a venue match is never flagged.
    Runs on the literal-neutralized code view, so string/char braces cannot
    confuse the depth tracking."""
    positions: list[int] = []
    n = len(scan_text)
    for kw in _MATCH_KW.finditer(scan_text):
        # locate the head's opening brace at paren-depth 0
        j = kw.end()
        paren = 0
        brace = -1
        while j < n:
            ch = scan_text[j]
            if ch == "(":
                paren += 1
            elif ch == ")":
                paren -= 1
            elif ch == ";" and paren == 0:
                break  # not a match-expression head
            elif ch == "{" and paren == 0:
                brace = j
                break
            j += 1
        if brace == -1:
            continue
        if not _NAME_RE.search(scan_text[kw.end() : brace]):
            continue
        # walk the body, isolating each arm's pattern (text before `=>`, minus
        # any `if` guard) and flagging venue literals found there.
        depth = 0
        k = brace + 1
        arm_start = k
        in_pattern = True
        while k < n:
            ch = scan_text[k]
            if ch in "([{":
                depth += 1
            elif ch in ")]":
                depth -= 1
            elif ch == "}":
                if depth == 0:
                    break  # end of the match body
                depth -= 1
                if depth == 0 and not in_pattern:
                    # A block-bodied arm (`pat => { .. }`) closes its body here
                    # and, per Rust, needs no trailing comma. The next character
                    # begins a new arm pattern, so resume pattern context;
                    # otherwise `in_pattern` would stay False and the following
                    # arm's pattern would never be scanned (fence false-negative).
                    in_pattern = True
                    arm_start = k + 1
            elif depth == 0 and in_pattern and ch == "=" and k + 1 < n and scan_text[k + 1] == ">":
                pattern = _GUARD.split(scan_text[arm_start:k], maxsplit=1)[0]
                for lit in _LIT_RE.finditer(pattern):
                    positions.append(arm_start + lit.start())
                in_pattern = False
                k += 2
                continue
            elif depth == 0 and not in_pattern and ch == ",":
                in_pattern = True
                arm_start = k + 1
            k += 1
    return positions


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    scan_text = _canonical_code(text)
    found: list[tuple[int, str]] = []
    for rule in FORBIDDEN_RULES:
        for match in rule.pattern.finditer(scan_text):
            found.append((match.start(), rule.label))
    for pos in _match_arm_positions(scan_text):
        found.append((pos, _MATCH_ARM_LABEL))

    violations: list[Violation] = []
    for pos, label in found:
        line_start = scan_text.rfind("\n", 0, pos) + 1
        line_end = scan_text.find("\n", pos)
        if line_end == -1:
            line_end = len(text)
        violations.append(
            Violation(
                path=path,
                line=line_number(scan_text, pos),
                label=label,
                excerpt=text[line_start:line_end].strip(),
            )
        )
    violations.sort(key=lambda v: (v.line, v.label))
    return violations


def bolt_src_files() -> list[Path]:
    src_root = REPO_ROOT / "src"
    files: list[Path] = []
    for path in src_root.rglob("*.rs"):
        if path.is_symlink():
            raise ValueError(f"src contains a symlink: {path}")
        if not path.is_file():
            continue
        rel = path.relative_to(REPO_ROOT).as_posix()
        if rel.startswith(PROVIDERS_PREFIX):
            continue
        files.append(path)
    files.sort(key=lambda path: path.relative_to(REPO_ROOT).as_posix().encode("utf-8"))
    return files


def collect_violations_from_files(files: list[Path]) -> list[Violation]:
    if not files:
        raise RuntimeError("no Rust source files found under src")
    violations: list[Violation] = []
    for path in files:
        rel = str(path.relative_to(REPO_ROOT))
        violations.extend(find_violations_in_text(rel, production_text(path)))
    return violations


def collect_violations() -> list[Violation]:
    return collect_violations_from_files(bolt_src_files())


def main() -> int:
    violations = collect_violations()
    if violations:
        for violation in violations:
            print(
                "FAIL: Bolt-v3 FR-080 venue-name branch fence "
                f"{violation.label} at {violation.path}:{violation.line}: {violation.excerpt}",
                file=sys.stderr,
            )
        return 1
    print("OK: Bolt-v3 FR-080 venue-name branch fence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
