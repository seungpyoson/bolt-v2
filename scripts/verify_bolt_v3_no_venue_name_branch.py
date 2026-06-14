#!/usr/bin/env python3
"""FR-080: forbid venue-name string-literal branches outside provider modules.

The capability contract (D8/FR-080) requires the controller to branch on venue
*capabilities* read from `VenueContract`, never on a hardcoded venue name. The
existing `verify_bolt_v3_core_boundary.py` catches only `match venue.kind` /
`VenueKind` enum dispatch over a fixed file set; it does NOT catch string-literal
comparisons like `venue_id == "polymarket"`. This fence closes that gap. Provider
modules under `src/bolt_v3_providers/` are exempt — that is where venue-name KEY
literals legitimately live.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

from bolt_v3_source_roots import REPO_ROOT
from verify_bolt_v3_pure_rust_runtime import production_text

PROVIDERS_PREFIX = "src/bolt_v3_providers/"

_VENUE = r"(?:polymarket|binance|bybit|okx|hyperliquid|deribit|chainlink|gamma)"
# A venue-name READ expression: an optional dotted receiver path, the venue-name
# token, then any reader-accessor calls. Built to (a) NOT match `venue` glued
# inside a longer identifier (`myvenue`, `subvenue`) and (b) consume a trailing
# accessor call so `venue.venue_id() == "x"` and `matches!(venue.as_str(), "x")`
# are caught, not just the bare-field forms.
_NAME = (
    r"(?<![A-Za-z0-9_])"  # leading boundary: reject `venue` inside `myvenue`/`subvenue`
    r"(?:[A-Za-z_][A-Za-z0-9_]*\.)*"  # optional dotted receiver path (self., foo.bar.)
    r"(?:venue_id|venue_name|venue)\b"  # the venue-name token
    r"(?:\s*\(\s*\))?"  # optional empty accessor call: venue_id()
    r"(?:\s*\.\s*[A-Za-z_][A-Za-z0-9_]*\s*\(\s*\))*"  # reader-accessor chain: .as_str().value()
)
_LIT = rf'"{_VENUE}"'


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
    Rule("venue-name equality (name == lit)", re.compile(rf"{_NAME}\s*==\s*{_LIT}", re.IGNORECASE)),
    Rule("venue-name equality (lit == name)", re.compile(rf"{_LIT}\s*==\s*{_NAME}", re.IGNORECASE)),
    Rule(
        "venue-name membership/method",
        re.compile(
            rf"{_NAME}\s*\.\s*(?:contains|starts_with|ends_with|eq|eq_ignore_ascii_case)\s*\(\s*{_LIT}",
            re.IGNORECASE,
        ),
    ),
    Rule("venue-name matches! arm", re.compile(rf"matches!\s*\(\s*{_NAME}\s*,[^)]*{_LIT}", re.IGNORECASE)),
    # A venue literal used directly as a `match` arm pattern (`"polymarket" => ...`)
    # is always a venue-name branch regardless of the scrutinee, so this rule is
    # anchored on the literal alone (no `_NAME` operand needed). A non-venue arm
    # (`"foo" => ...`) cannot match because `_LIT` is constrained to venue names.
    Rule("venue-name match arm (lit =>)", re.compile(rf"{_LIT}\s*=>", re.IGNORECASE)),
)

_COMMENT_OR_STRING = re.compile(r'"(?:\\.|[^"\\])*"|//[^\n]*|/\*.*?\*/', re.DOTALL)


def strip_comments_keep_strings(text: str) -> str:
    """Blank // and /* */ comments but PRESERVE string literals and newlines.

    String literals are matched first in the alternation, so a `//` or `/*`
    inside a string is consumed as part of the (preserved) literal. Comments are
    replaced char-for-char with spaces so byte offsets and line numbers are
    unchanged.
    """

    def repl(match: re.Match[str]) -> str:
        token = match.group(0)
        if token.startswith('"'):
            return token
        return re.sub(r"[^\n]", " ", token)

    return _COMMENT_OR_STRING.sub(repl, text)


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    scan_text = strip_comments_keep_strings(text)
    violations: list[Violation] = []
    for rule in FORBIDDEN_RULES:
        for match in rule.pattern.finditer(scan_text):
            line_start = scan_text.rfind("\n", 0, match.start()) + 1
            line_end = scan_text.find("\n", match.end())
            if line_end == -1:
                line_end = len(text)
            violations.append(
                Violation(
                    path=path,
                    line=line_number(scan_text, match.start()),
                    label=rule.label,
                    excerpt=text[line_start:line_end].strip(),
                )
            )
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
