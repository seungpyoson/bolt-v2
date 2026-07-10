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

  * Every string literal *spelling* is normalized by `_canonical_code` before
    any rule runs — plain, raw, byte, byte-raw, C, and C-raw literals all
    collapse to a plain empty-string token, so a prefix cannot slip a literal
    past the branch rules.
  * Balanced turbofish contents and transparent grouping parentheses are
    normalized before matching while call-argument parentheses remain intact.
  * Comments, char literals, and string-literal bodies are blanked (newlines
    preserved), so a venue-branch phrase living inside a raw-string body or a
    comment is NOT scanned as code (no false positive).
  * `match` arms are flagged only when (a) the match scrutinee is itself a venue
    read AND (b) the venue literal sits in arm-*pattern* position at the match's
    top level — never in an arm body, an `if`-guard expression, or a nested
    `match` on a non-venue scrutinee. So `match mode { "gamma" => .. }` and
    `match venue { _ => { match mode { "gamma" => .. } } }` are both clean.

KNOWN, DELIBERATELY-UNCAUGHT forms (they need type/flow analysis a regex cannot
do soundly; the capability-contract design + code review cover them):
  * constructed/split literals: `concat!("poly","market")`, `format!(..)`;
  * wrapper/alias PartialEq where neither operand is a syntactic venue read:
    `VenueId::from("polymarket") == other`, `some_non_venue_var == "polymarket"`;
  * venue-name read via a `venue`-*suffixed* getter (`obj.get_venue()`) rather
    than a `venue`-prefixed token;
  * a `match`/`if let` guard comparing a non-venue operand to a venue literal.
These are accepted false negatives, recorded here so the boundary is explicit.
"""

from __future__ import annotations

import posixpath
import re
import sys
from dataclasses import dataclass
from pathlib import Path

from bolt_v3_source_roots import REPO_ROOT
from verify_bolt_v3_provider_leaks import production_text as production_source_text
from verifier_io import require_nonempty

PROVIDERS_PREFIX = "src/bolt_v3_providers/"


def production_text(path: Path) -> str:
    return production_source_text(path.read_text(encoding="utf-8"))


# A venue-identity READ expression: an optional dotted receiver path, a
# `venue`-prefixed token (so `venue_id`, `venue_name`, `venue_wrapper` all
# qualify), then any zero-arg reader-accessor calls incl. turbofish
# (`.as_str()`, `.as_ref::<str>()`, `.cast::<Cow<str>>()`). The leading boundary
# stops `venue` from matching inside a larger identifier (`subvenue`,
# `revenue`, `myvenue`). Receiver/accessor repetition is bounded so a
# pathological line cannot drive quadratic backtracking.
_TURBOFISH = r"(?:\s*::\s*<[^<>]*>)?"
_NAME = (
    r"(?<![A-Za-z0-9_])"
    r"(?:[A-Za-z_][A-Za-z0-9_]*\s*\.\s*){0,16}"
    r"venue[A-Za-z0-9_]*"
    + _TURBOFISH
    + r"(?:\s*\(\s*\))?"
    r"(?:\s*\.\s*[A-Za-z_][A-Za-z0-9_]*" + _TURBOFISH + r"\s*\(\s*\)){0,16}"
)
# After `_canonical_code` every string literal is represented by a plain
# double-quoted token. The literal side deliberately accepts any contents; it
# does not copy or enumerate provider venue keys.
_LIT = r'"[^"\n]*"'
_EQ = r"(?:==|!=)"


@dataclass(frozen=True)
class Rule:
    rule_id: str
    label: str
    pattern: re.Pattern[str]


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    rule_id: str
    label: str
    excerpt: str

    def key(self) -> tuple[str, str]:
        return (self.rule_id, normalize_path(self.path))


FORBIDDEN_RULES = (
    Rule(
        "FR080_EQ_NAME_LIT",
        "venue-name equality (name eq lit)",
        re.compile(rf"{_NAME}\s*{_EQ}\s*{_LIT}", re.IGNORECASE),
    ),
    Rule(
        "FR080_EQ_LIT_NAME",
        "venue-name equality (lit eq name)",
        re.compile(rf"{_LIT}\s*{_EQ}\s*{_NAME}", re.IGNORECASE),
    ),
    Rule(
        "FR080_METHOD",
        "venue-name membership/method",
        re.compile(
            rf"{_NAME}\s*\.\s*(?:contains|starts_with|ends_with|eq|eq_ignore_ascii_case)"
            rf"{_TURBOFISH}\s*\(\s*(?:&\s*)?{_LIT}",
            re.IGNORECASE,
        ),
    ),
)

_MATCH_KW = re.compile(r"\bmatch\b")
_MATCHES_KW = re.compile(r"\bmatches!\s*([([{])")
_CONDITION_KW = re.compile(r"\b(?:if|while)\b")
_LET_KW = re.compile(r"\blet\b")
_MATCH_HEAD_RE = re.compile(
    rf"(?:(?:&\s*(?:mut\s+)?)|(?:\(\s*))*{_NAME}(?:\s*\))*",
    re.IGNORECASE,
)
_LIT_RE = re.compile(_LIT, re.IGNORECASE)
_NAME_RE = re.compile(_NAME, re.IGNORECASE)
_GUARD = re.compile(r"\bif\b")
_MATCH_ARM_LABEL = "venue-name match arm (venue scrutinee)"
_MATCH_ARM_RULE_ID = "FR080_MATCH_ARM"
_MATCHES_LABEL = "venue-name matches! arm"
_MATCHES_RULE_ID = "FR080_MATCHES"
_LET_PATTERN_LABEL = "venue-name if/while-let"
_LET_PATTERN_RULE_ID = "FR080_LET_PATTERN"
_DISCOVERY_FLOOR_RULE_ID = "FR080_DISCOVERY_FLOOR"

# A complete Rust char literal: `'a'`, `'\n'`, `'\''`, `'\x41'`, `'\u{1F600}'`,
# `'"'`. Deliberately does NOT match a lifetime (`'a` with no closing quote).
_CHAR = re.compile(r"'(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]+\}|.)|[^\\'\n])'")
_IDENT = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_")


def _scan_plain(text: str, quote: int, start: int) -> tuple[int, int, str | None]:
    """Scan a plain/byte double-quoted string. `quote` = opening `"`, `start` =
    literal start (may precede `quote` for a `b"` prefix). Returns
    (start, end_exclusive, value) — value is None only when the literal is
    unterminated. Escapes remain opaque because canonicalization needs only to
    distinguish a complete literal from incomplete source."""
    n = len(text)
    j = quote + 1
    while j < n:
        ch = text[j]
        if ch == "\\":
            j += 2
            continue
        if ch == '"':
            body = text[quote + 1 : j]
            return (start, j + 1, body)
        if ch == "\n":
            return (start, j, None)  # unterminated at EOL
        j += 1
    return (start, n, None)  # unterminated at EOF


def _scan_string(text: str, i: int) -> tuple[int, int, str | None] | None:
    """If a string literal begins at index `i`, return (start, end_exclusive,
    value); else None. Handles plain, raw, byte, and C string literals,
    including hashed raw spellings. `value` is the opaque source content, or
    None when the literal is unterminated."""
    n = len(text)
    c = text[i]
    if c == '"':
        return _scan_plain(text, i, i)
    # Raw/byte/C prefixes are string starts only when not an identifier tail.
    if c in ("r", "b", "c") and (i == 0 or text[i - 1] not in _IDENT):
        j = i
        has_plain_prefix = text[j] in ("b", "c")
        if has_plain_prefix:
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
                return (i, k + len(closing), body)
            return None  # raw prefix not followed by a raw string body
        if has_plain_prefix and j < n and text[j] == '"':
            return _scan_plain(text, j, i)
        return None
    return None


def _flatten_turbofish(text: str) -> str:
    """Blank balanced turbofish contents while retaining the outer `<...>`."""
    out = list(text)
    n = len(text)
    i = 0
    while i + 2 < n:
        if text[i : i + 2] != "::":
            i += 1
            continue
        opening = i + 2
        while opening < n and text[opening].isspace():
            opening += 1
        if opening >= n or text[opening] != "<":
            i += 2
            continue
        depth = 1
        delimiters: list[str] = []
        j = opening + 1
        while j < n and depth:
            ch = text[j]
            if ch in "([{":
                delimiters.append({"(": ")", "[": "]", "{": "}"}[ch])
            elif ch in ")]}" and delimiters and ch == delimiters[-1]:
                delimiters.pop()
            elif not delimiters and ch == "<":
                depth += 1
            elif not delimiters and ch == ">" and text[j - 1] != "-":
                depth -= 1
            j += 1
        if depth:
            break
        for k in range(opening + 1, j - 1):
            if out[k] != "\n":
                out[k] = " "
        i = j
    return "".join(out)


def _group_prefix_allows_transparency(chars: list[str], opening: int) -> bool:
    i = opening - 1
    while i >= 0 and chars[i].isspace():
        i -= 1
    if i < 0:
        return True
    if chars[i] not in _IDENT:
        return chars[i] not in "!)]>"
    end = i + 1
    while i >= 0 and chars[i] in _IDENT:
        i -= 1
    if i >= 0 and chars[i] == "#":
        return False
    return "".join(chars[i + 1 : end]) in {
        "break",
        "else",
        "if",
        "let",
        "match",
        "move",
        "return",
        "while",
        "yield",
    }


def _flatten_transparent_groups(text: str) -> str:
    """Blank grouping parens around a complete venue read or string literal."""
    out = list(text)
    stack: list[int] = []
    pairs: list[tuple[int, int]] = []
    for i, ch in enumerate(text):
        if ch == "(":
            stack.append(i)
        elif ch == ")" and stack:
            pairs.append((stack.pop(), i))
    for opening, closing in pairs:
        inner = "".join(out[opening + 1 : closing]).strip()
        if not _group_prefix_allows_transparency(out, opening):
            continue
        if _NAME_RE.fullmatch(inner) or _LIT_RE.fullmatch(inner):
            out[opening] = " "
            out[closing] = " "
    return "".join(out)


def _canonical_code(text: str) -> str:
    """Length-preserving code view with every string normalized to `""`.

    Comments, char literals, and string bodies are blanked while newlines stay
    aligned. Keeping a quote pair at each string start lets the rules match any
    literal without knowing or copying its value.
    """
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
            depth = 1
            j = i + 2
            while j < n and depth:
                if text[j : j + 2] == "/*":
                    depth += 1
                    j += 2
                elif text[j : j + 2] == "*/":
                    depth -= 1
                    j += 2
                else:
                    j += 1
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
        lit = _scan_string(text, i)
        if lit is not None:
            start, end, value = lit
            blank(start, end)
            if value is not None:
                out[start] = '"'
                out[start + 1] = '"'
            i = end
            continue
        i += 1

    return _flatten_transparent_groups(_flatten_turbofish("".join(out)))


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def normalize_path(path: str) -> str:
    return posixpath.normpath(path.replace("\\", "/"))


_OPEN_TO_CLOSE = {"(": ")", "[": "]", "{": "}"}
_CLOSERS = frozenset(_OPEN_TO_CLOSE.values())


def _closing_delimiter(text: str, opening: int) -> int | None:
    stack = [_OPEN_TO_CLOSE[text[opening]]]
    for i in range(opening + 1, len(text)):
        ch = text[i]
        if ch in _OPEN_TO_CLOSE:
            stack.append(_OPEN_TO_CLOSE[ch])
        elif ch in _CLOSERS:
            if ch != stack[-1]:
                return None
            stack.pop()
            if not stack:
                return i
    return None


def _top_level_comma(text: str, start: int, end: int) -> int | None:
    stack: list[str] = []
    for i in range(start, end):
        ch = text[i]
        if ch in _OPEN_TO_CLOSE:
            stack.append(_OPEN_TO_CLOSE[ch])
        elif ch in _CLOSERS:
            if stack and ch == stack[-1]:
                stack.pop()
        elif ch == "," and not stack:
            return i
    return None


def _top_level_guard(text: str, start: int, end: int) -> int | None:
    stack: list[str] = []
    i = start
    while i < end:
        ch = text[i]
        if ch in _OPEN_TO_CLOSE:
            stack.append(_OPEN_TO_CLOSE[ch])
        elif ch in _CLOSERS:
            if stack and ch == stack[-1]:
                stack.pop()
        elif not stack and text.startswith("if", i):
            before = text[i - 1] if i > start else " "
            after = text[i + 2] if i + 2 < end else " "
            if before not in _IDENT and after not in _IDENT:
                return i
        i += 1
    return None


def _matches_positions(scan_text: str) -> list[int]:
    positions: list[int] = []
    for macro in _MATCHES_KW.finditer(scan_text):
        opening = macro.end() - 1
        closing = _closing_delimiter(scan_text, opening)
        if closing is None:
            continue
        comma = _top_level_comma(scan_text, opening + 1, closing)
        if comma is None:
            continue
        scrutinee = scan_text[opening + 1 : comma].strip()
        if not _MATCH_HEAD_RE.fullmatch(scrutinee):
            continue
        pattern_start = comma + 1
        guard = _top_level_guard(scan_text, pattern_start, closing)
        pattern_end = closing if guard is None else guard
        positions.extend(
            pattern_start + literal.start()
            for literal in _LIT_RE.finditer(scan_text[pattern_start:pattern_end])
        )
    return positions


def _condition_end(scan_text: str, start: int) -> int:
    stack: list[str] = []
    last_nonspace = ""
    i = start
    while i < len(scan_text):
        ch = scan_text[i]
        if ch in "([":
            stack.append(_OPEN_TO_CLOSE[ch])
        elif ch == "{" and (stack or last_nonspace == "!"):
            stack.append("}")
        elif ch in _CLOSERS:
            if stack and ch == stack[-1]:
                stack.pop()
        elif not stack and (ch == "{" or ch == ";"):
            return i
        if not ch.isspace():
            last_nonspace = ch
        i += 1
    return len(scan_text)


def _top_level_assignment(text: str, start: int, end: int) -> int | None:
    stack: list[str] = []
    for i in range(start, end):
        ch = text[i]
        if ch in _OPEN_TO_CLOSE:
            stack.append(_OPEN_TO_CLOSE[ch])
        elif ch in _CLOSERS:
            if stack and ch == stack[-1]:
                stack.pop()
        elif ch == "=" and not stack:
            before = text[i - 1] if i > start else " "
            after = text[i + 1] if i + 1 < end else " "
            if before not in "!<=>" and after not in "=>":
                return i
    return None


def _let_rhs_end(text: str, start: int, end: int) -> int:
    stack: list[str] = []
    prefix_only_borrows = True
    i = start
    while i < end:
        ch = text[i]
        if ch in _OPEN_TO_CLOSE:
            stack.append(_OPEN_TO_CLOSE[ch])
        elif ch in _CLOSERS:
            if stack and ch == stack[-1]:
                stack.pop()
        elif not stack and text[i : i + 2] in ("&&", "||"):
            unary_borrow = text[i : i + 2] == "&&" and prefix_only_borrows
            if not unary_borrow:
                return i
            i += 2
            continue
        if ch not in "& \t\r\n":
            prefix_only_borrows = False
        i += 1
    return end


def _let_pattern_positions(scan_text: str) -> list[int]:
    positions: list[int] = []
    for condition in _CONDITION_KW.finditer(scan_text):
        end = _condition_end(scan_text, condition.end())
        for let_kw in _LET_KW.finditer(scan_text, condition.end(), end):
            assignment = _top_level_assignment(scan_text, let_kw.end(), end)
            if assignment is None:
                continue
            rhs_end = _let_rhs_end(scan_text, assignment + 1, end)
            if not _MATCH_HEAD_RE.fullmatch(scan_text[assignment + 1 : rhs_end].strip()):
                continue
            positions.extend(
                let_kw.end() + literal.start()
                for literal in _LIT_RE.finditer(scan_text[let_kw.end() : assignment])
            )
    return positions


def _next_top_level_token_is_arm(scan_text: str, start: int) -> bool:
    """Return whether a comma-less block body is followed by another arm."""
    stack: list[str] = []
    i = start
    while i < len(scan_text):
        ch = scan_text[i]
        if ch in _OPEN_TO_CLOSE:
            stack.append(_OPEN_TO_CLOSE[ch])
        elif ch in _CLOSERS:
            if stack and ch == stack[-1]:
                stack.pop()
            elif not stack:
                return False
        elif not stack:
            if ch == ",":
                return False
            if scan_text[i : i + 2] == "=>":
                return True
        i += 1
    return False


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
        if not _MATCH_HEAD_RE.fullmatch(scan_text[kw.end() : brace].strip()):
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
                if (
                    depth == 0
                    and not in_pattern
                    and _next_top_level_token_is_arm(scan_text, k + 1)
                ):
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
    path = normalize_path(path)
    if path.startswith(PROVIDERS_PREFIX):
        return []
    scan_text = _canonical_code(text)
    found: list[tuple[int, str, str]] = []
    for rule in FORBIDDEN_RULES:
        for match in rule.pattern.finditer(scan_text):
            found.append((match.start(), rule.rule_id, rule.label))
    for pos in _matches_positions(scan_text):
        found.append((pos, _MATCHES_RULE_ID, _MATCHES_LABEL))
    for pos in _let_pattern_positions(scan_text):
        found.append((pos, _LET_PATTERN_RULE_ID, _LET_PATTERN_LABEL))
    for pos in _match_arm_positions(scan_text):
        found.append((pos, _MATCH_ARM_RULE_ID, _MATCH_ARM_LABEL))

    violations: list[Violation] = []
    seen_keys: set[tuple[str, str]] = set()
    for pos, rule_id, label in found:
        line_start = scan_text.rfind("\n", 0, pos) + 1
        line_end = scan_text.find("\n", pos)
        if line_end == -1:
            line_end = len(text)
        violation = Violation(
            path=path,
            line=line_number(scan_text, pos),
            rule_id=rule_id,
            label=label,
            excerpt=text[line_start:line_end].strip(),
        )
        if violation.key() in seen_keys:
            continue
        seen_keys.add(violation.key())
        violations.append(violation)
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
    floor_errors: list[str] = []
    if not require_nonempty(files, "Rust source files under src", floor_errors):
        return [
            Violation(path=".", line=0, rule_id=_DISCOVERY_FLOOR_RULE_ID, label=error, excerpt="")
            for error in floor_errors
        ]
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
