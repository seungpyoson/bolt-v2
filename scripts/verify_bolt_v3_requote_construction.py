#!/usr/bin/env python3
"""Verify the maker requote-budget governor is constructed only via the config bridge.

`bolt_v3_maker_rate_budget::build_requote_budget_pair` is the single no-hardcodes
construction path for the maker's two-budget requote governor: it sources the
submit-command budget from `risk.nautilus.max_order_submit_rate` (through the one
shared rate parser) and the REST budget from `VenueEgressModel::cap_per_minute`, and
it fails closed on every degenerate input (a REST cap below the cancel+resubmit
reprice cost, a zero anti-flicker interval, a malformed or millisecond-overflowing
rate string).

If production code calls `RequoteBudgetPair::new` or the inner `RequoteBudget::new`
directly, it can pass hardcoded caps/windows and bypass every one of those guards
plus the config sourcing — a NO-HARDCODES and NO-DUAL-PATHS violation that turns the
bridge into a suggestion instead of THE construction path. This fence forbids
`RequoteBudget[Pair]::new(` call-sites in production (non-`#[cfg(test)]`) Rust source
anywhere except the bridge module itself. `#[cfg(test)]` fixtures are exempt: tests
legitimately construct budgets with explicit caps to exercise the gate.

Three evasion forms beyond the bare call-site are also closed, so the fence is not
defeated by trivial rephrasing:
- WHITESPACE / UFCS: `RequoteBudget :: new(` and `<RequoteBudgetPair as T>::new(`
  compile but spell the path differently; the constructor pattern tolerates
  whitespace around `::` and matches the fully-qualified `<Type>::new(` form.
- ALIAS: `use ...::RequoteBudget as Foo;` then `Foo::new(...)` carries no
  `RequoteBudget` token at the call-site, so the only catchable signal is the alias
  import itself. Any `as`-rename of the governor type outside the bridge is a
  violation (mirrors `verify_bolt_v3_provider_leaks.py`'s `use ... as` clause). A
  plain unaliased `use ...::RequoteBudgetPair;` to name the type in a signature is
  allowed; only the `as <ident>` rename is forbidden.
- VISIBILITY: the `src/`-only scan scope is sound ONLY while both constructors stay
  `pub(crate)` — `pub(crate)` compile-blocks construction from the separate
  backtesting-vertical-slice crate and the `tests/` integration crates, so those
  units need no scan. Widening either `new` back to bare `pub` would silently
  reopen that cross-crate path with no fence coverage, so this fence also fails if
  a bare `pub fn new` appears in the defining module, binding the two halves of the
  dual-path closure together.

Scope boundary (intentional, hand-reviewed): construction WITHIN the defining
module via `Self { .. }` / `Self::new(..)` cannot be policed by any textual scan —
a type's own module can always name its private fields — exactly as
`verify_bolt_v3_provider_leaks.py` trusts the provider module's own definitions.
That surface is one small primitive file reviewed by hand. A pure compile-time seal
(relocating the governor type into the bridge module so `new` is module-private)
was deliberately NOT adopted: the bridge depends on `bolt_v3_validate` for rate-string
parsing, so co-locating the primitive there would invert layering (a pure primitive
gaining a config-orchestrator dependency). Keeping the primitive in its own module
and enforcing single-construction via this fence matches the repo's established
source-fence idiom for cross-module invariants that are not compile-expressible.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

from bolt_v3_source_roots import REPO_ROOT
from verify_bolt_v3_pure_rust_runtime import (
    production_text,
    strip_rust_comments_and_literals,
)
from verifier_io import require_nonempty

# The single production module permitted to construct the budget primitives: the
# config bridge that sources every cap/window from TOML + venue capability facts.
# Path is relative to REPO_ROOT, POSIX-normalized.
BRIDGE_PATH = "src/bolt_v3_maker_rate_budget.rs"

# The module that DEFINES the budget primitives. Its constructors must stay
# `pub(crate)` so the cross-crate construction path is compile-blocked (see module
# docstring, VISIBILITY). Path relative to REPO_ROOT, POSIX-normalized.
DEFINING_PATH = "src/bolt_v3_requote_budget.rs"

# Direct/whitespace `RequoteBudget[Pair] :: new(` plus the fully-qualified
# `<RequoteBudget[Pair] as Trait>::new(` (UFCS) form. Whitespace is tolerated around
# `::` and before `(` because rustfmt is not guaranteed to have run before the fence.
CONSTRUCTOR_PATTERN = re.compile(
    r"(?<![A-Za-z0-9_])RequoteBudget(?:Pair)?\s*::\s*new\s*\("
    r"|<\s*RequoteBudget(?:Pair)?(?:\s+as\s+[^>]+?)?\s*>\s*::\s*new\s*\("
)

# An `as`-rename of the governor type outside the bridge: `use ...RequoteBudget as
# Foo;`. Once renamed, `Foo::new(...)` carries no `RequoteBudget` token, so the alias
# import is the only catchable signal. A plain unaliased `use ...RequoteBudgetPair;`
# (no `as`) is allowed — only the rename is forbidden.
ALIAS_PATTERN = re.compile(
    r"\b(?:pub\s+)?use\s+[^;]*\bRequoteBudget(?:Pair)?\b\s+as\s+[A-Za-z_]"
)

# A bare `pub fn new` in the defining module (the constructors must stay
# `pub(crate)`; `pub(crate) fn new` does not match because `pub` is followed by `(`).
PUBLIC_NEW_PATTERN = re.compile(r"\bpub[ \t]+fn[ \t]+new\b")

# Scan rules applied to every non-bridge production source file. Each maps a pattern
# to the violation `kind` so `main` can print the right remediation.
SCAN_RULES = (
    (CONSTRUCTOR_PATTERN, "construct"),
    (ALIAS_PATTERN, "alias"),
)


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    excerpt: str
    kind: str = "construct"


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def bolt_src_files() -> list[Path]:
    src_root = REPO_ROOT / "src"
    files: list[Path] = []
    for path in src_root.rglob("*.rs"):
        if path.is_symlink():
            raise ValueError(f"src contains a symlink: {path}")
        if path.is_file():
            files.append(path)
    files.sort(key=lambda path: path.relative_to(REPO_ROOT).as_posix().encode("utf-8"))
    return files


def _excerpt(text: str, start: int, end: int) -> str:
    line_start = text.rfind("\n", 0, start) + 1
    line_end = text.find("\n", end)
    if line_end == -1:
        line_end = len(text)
    return text[line_start:line_end].strip()


def find_violations_in_text(rel: str, text: str) -> list[Violation]:
    scan_text = strip_rust_comments_and_literals(text)
    violations: list[Violation] = []
    for pattern, kind in SCAN_RULES:
        for match in pattern.finditer(scan_text):
            violations.append(
                Violation(
                    path=rel,
                    line=line_number(scan_text, match.start()),
                    excerpt=_excerpt(text, match.start(), match.end()),
                    kind=kind,
                )
            )
    violations.sort(key=lambda violation: (violation.line, violation.kind))
    return violations


def find_visibility_violations_in_text(rel: str, text: str) -> list[Violation]:
    scan_text = strip_rust_comments_and_literals(text)
    violations: list[Violation] = []
    for match in PUBLIC_NEW_PATTERN.finditer(scan_text):
        violations.append(
            Violation(
                path=rel,
                line=line_number(scan_text, match.start()),
                excerpt=_excerpt(text, match.start(), match.end()),
                kind="visibility",
            )
        )
    return violations


def collect_violations_from_files(files: list[Path]) -> list[Violation]:
    floor_errors: list[str] = []
    if not require_nonempty(files, "Rust source files under src", floor_errors):
        return [
            Violation(path=".", line=0, excerpt=error, kind="source-floor")
            for error in floor_errors
        ]

    violations: list[Violation] = []
    for path in files:
        rel = path.relative_to(REPO_ROOT).as_posix()
        if rel == BRIDGE_PATH:
            continue
        violations.extend(find_violations_in_text(rel, production_text(path)))
    return violations


def collect_violations() -> list[Violation]:
    return collect_violations_from_files(bolt_src_files())


def collect_visibility_violations() -> list[Violation]:
    defining = REPO_ROOT / DEFINING_PATH
    if not defining.is_file():
        raise RuntimeError(
            f"requote-budget defining module missing: {DEFINING_PATH}"
        )
    return find_visibility_violations_in_text(DEFINING_PATH, production_text(defining))


def main() -> int:
    violations = collect_violations()
    visibility = []
    if not any(violation.kind == "source-floor" for violation in violations):
        visibility = collect_visibility_violations()
    if violations or visibility:
        for violation in violations:
            if violation.kind == "source-floor":
                print(
                    "FAIL: Bolt-v3 requote-budget construction fence: "
                    f"{violation.excerpt}",
                    file=sys.stderr,
                )
            elif violation.kind == "alias":
                print(
                    "FAIL: Bolt-v3 requote-budget construction fence: "
                    f"{violation.path}:{violation.line} aliases the requote governor "
                    f"type with `as` (an alias evades the construction fence); import "
                    f"RequoteBudget[Pair] unaliased and construct via "
                    f"build_requote_budget_pair: {violation.excerpt}",
                    file=sys.stderr,
                )
            else:
                print(
                    "FAIL: Bolt-v3 requote-budget construction fence: "
                    f"{violation.path}:{violation.line} constructs the requote governor "
                    f"outside the config bridge ({BRIDGE_PATH}); route construction "
                    f"through build_requote_budget_pair so caps/windows stay "
                    f"config-sourced: {violation.excerpt}",
                    file=sys.stderr,
                )
        for violation in visibility:
            print(
                "FAIL: Bolt-v3 requote-budget construction fence: "
                f"{violation.path}:{violation.line} widens a requote-governor "
                f"constructor to bare `pub`; it must stay `pub(crate)` so the "
                f"src/-only fence scope keeps the cross-crate construction path "
                f"compile-blocked: {violation.excerpt}",
                file=sys.stderr,
            )
        return 1

    print("OK: Bolt-v3 requote-budget construction fence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
