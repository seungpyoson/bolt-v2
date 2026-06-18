#!/usr/bin/env python3
"""Verify `UsableMu` is minted only by the μ health gate.

`UsableMu` (`src/bolt_v3_maker_mu_estimator.rs`) is the newtype that carries a
gate-cleared informed-fraction μ into the maker quote planner. Its private field
and `pub(crate) fn new` already make an *accidental* bare-`f64`
`MakerQuotePlanInputs { informed_fraction: 0.5, .. }` a hard compile error. But a
`pub(crate)` constructor leaves a *deliberate* in-crate mint outside the gate
possible, which would route an ungated μ around the `MuHealthReason` checks (the
exact class MU-3 closes: "an ungated μ reaches the planner"). This fence makes
the sole-mint property structural, not convention: the only production mint of a
`UsableMu` is the gate `MakerMuState::usable_mu_for`
(`src/strategies/binary_oracle_maker/mu.rs`), and only inside that one function.

The fence closes the CLASS of bypass, not one form:
  1. A `UsableMu::new` mint (call, `.map` function-reference, or raw-ident
     `r#new`) anywhere in production `src/` is a violation, EXCEPT inside the
     body of `usable_mu_for` in the gate file. A rogue mint elsewhere in the gate
     file (e.g. a second `pub fn` that mints) fails — the exemption is scoped to
     the gate function span, not the whole file.
  2. A rename evades a literal `UsableMu::new` regex, so any production
     `use …UsableMu as <Alias>;` import-rename and any `type <X> = UsableMu;`
     type alias is itself a violation (no legitimate production reason to rename
     the gated newtype), AND a mint through the captured alias (`<Alias>::new`)
     is flagged too.
  3. `new` must stay the only constructor: a `From`/`Default`/`Deserialize` impl
     for `UsableMu`, or any non-`new` associated fn returning `Self`/`UsableMu`,
     would be a structural mint surface and is flagged.

`#[cfg(test)]` items are stripped before scanning (shared `production_text`
helper), so unit-test mints stay legal without a public bypass constructor.
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

# Matches the call form `UsableMu::new(...)`, the function-reference form
# `UsableMu::new` (e.g. `.map(UsableMu::new)`), and the raw-ident `UsableMu::r#new`;
# the trailing negative lookahead prevents matching `UsableMu::new_unchecked`.
USABLE_MU_NEW = re.compile(r"(?<![A-Za-z0-9_])UsableMu::(?:r#)?new(?![A-Za-z0-9_])")

# `use <path>UsableMu as <Alias>;` — a production import-rename of the gated
# newtype. Captures the alias so its `<Alias>::new` mints can be flagged too.
USABLE_MU_USE_ALIAS = re.compile(
    r"(?<![A-Za-z0-9_])use\s+[^;]*?(?<![A-Za-z0-9_])UsableMu\s+as\s+(?P<alias>[A-Za-z_][A-Za-z0-9_]*)"
)

# `type <Ident> = UsableMu;` — a production type alias of the gated newtype.
USABLE_MU_TYPE_ALIAS = re.compile(
    r"(?<![A-Za-z0-9_])type\s+(?P<alias>[A-Za-z_][A-Za-z0-9_]*)\s*=\s*UsableMu(?![A-Za-z0-9_])\s*;"
)

# Structural-mint surfaces on `UsableMu` other than `fn new`: trait impls that
# construct a `UsableMu` from outside the gate.
USABLE_MU_FROM_IMPL = re.compile(
    r"(?<![A-Za-z0-9_])impl\s+From<[^>]*>\s+for\s+UsableMu(?![A-Za-z0-9_])"
)
USABLE_MU_DEFAULT_IMPL = re.compile(
    r"(?<![A-Za-z0-9_])impl\s+Default\s+for\s+UsableMu(?![A-Za-z0-9_])"
)
USABLE_MU_DESERIALIZE_IMPL = re.compile(
    r"(?<![A-Za-z0-9_])impl(?:<[^>]*>)?\s+Deserialize(?:<[^>]*>)?\s+for\s+UsableMu(?![A-Za-z0-9_])"
)

# The μ health gate is the sole legitimate mint of `UsableMu`: `usable_mu_for`
# clears the `MuHealthReason` checks and only then maps the cleared value through
# `UsableMu::new`. Every other production mint is a bypass of that gate.
GATE_PATH = "src/strategies/binary_oracle_maker/mu.rs"
GATE_FN = "usable_mu_for"


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    excerpt: str
    rule: str


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def excerpt_at(text: str, pos: int) -> str:
    line_start = text.rfind("\n", 0, pos) + 1
    line_end = text.find("\n", pos)
    if line_end == -1:
        line_end = len(text)
    return text[line_start:line_end].strip()


def function_body_span(scan_text: str, fn_name: str) -> tuple[int, int] | None:
    """Char span [start, end) of `fn <fn_name>`'s body, braces included.

    Operates on comment/literal-stripped text so brace counting is sound (string
    and comment braces are blanked by `strip_rust_comments_and_literals`). Finds
    the `fn <fn_name>` token, advances to the first top-level `{` after it (the
    body open), and brace-matches to the close. Returns None if absent.
    """
    fn_token = re.compile(rf"(?<![A-Za-z0-9_])fn\s+{re.escape(fn_name)}(?![A-Za-z0-9_])")
    match = fn_token.search(scan_text)
    if match is None:
        return None
    open_brace = scan_text.find("{", match.end())
    if open_brace == -1:
        return None
    depth = 0
    i = open_brace
    while i < len(scan_text):
        char = scan_text[i]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return (open_brace, i + 1)
        i += 1
    return None


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    scan_text = strip_rust_comments_and_literals(text)
    violations: list[Violation] = []

    # Rule 1: gate function is the sole mint of `UsableMu::new`. In the gate file
    # the exemption is scoped to the `usable_mu_for` body span; elsewhere any mint
    # is a violation. Renamed mints (Rule 2) are matched via the alias set below.
    if path == GATE_PATH:
        span = function_body_span(scan_text, GATE_FN)
        if span is None:
            # Fail closed: if the gate function can't be located, every mint in
            # the file is unexempt (an unparseable gate is not a license to mint).
            span = (-1, -1)
        gate_start, gate_end = span
        for match in USABLE_MU_NEW.finditer(scan_text):
            if gate_start <= match.start() < gate_end:
                continue
            violations.append(
                Violation(
                    path=path,
                    line=line_number(scan_text, match.start()),
                    excerpt=excerpt_at(text, match.start()),
                    rule="UsableMu mint outside the usable_mu_for gate function",
                )
            )
    else:
        for match in USABLE_MU_NEW.finditer(scan_text):
            violations.append(
                Violation(
                    path=path,
                    line=line_number(scan_text, match.start()),
                    excerpt=excerpt_at(text, match.start()),
                    rule="UsableMu mint outside the gate file",
                )
            )

    # Rule 2: rename evasion. Flag the alias/type-alias declaration AND any mint
    # through the captured alias (`<Alias>::new`). No production code should
    # rename the gated newtype.
    aliases: set[str] = set()
    for match in USABLE_MU_USE_ALIAS.finditer(scan_text):
        aliases.add(match.group("alias"))
        violations.append(
            Violation(
                path=path,
                line=line_number(scan_text, match.start()),
                excerpt=excerpt_at(text, match.start()),
                rule="UsableMu import-renamed (alias evades the gate)",
            )
        )
    for match in USABLE_MU_TYPE_ALIAS.finditer(scan_text):
        aliases.add(match.group("alias"))
        violations.append(
            Violation(
                path=path,
                line=line_number(scan_text, match.start()),
                excerpt=excerpt_at(text, match.start()),
                rule="UsableMu type-aliased (alias evades the gate)",
            )
        )
    for alias in aliases:
        alias_new = re.compile(
            rf"(?<![A-Za-z0-9_]){re.escape(alias)}::(?:r#)?new(?![A-Za-z0-9_])"
        )
        for match in alias_new.finditer(scan_text):
            violations.append(
                Violation(
                    path=path,
                    line=line_number(scan_text, match.start()),
                    excerpt=excerpt_at(text, match.start()),
                    rule="UsableMu minted through an alias",
                )
            )

    # Rule 3: `new` must stay the only constructor — no From/Default/Deserialize
    # mint surface for `UsableMu`.
    for rule_re, label in (
        (USABLE_MU_FROM_IMPL, "From impl mints UsableMu outside the gate"),
        (USABLE_MU_DEFAULT_IMPL, "Default impl mints UsableMu outside the gate"),
        (USABLE_MU_DESERIALIZE_IMPL, "Deserialize impl mints UsableMu outside the gate"),
    ):
        for match in rule_re.finditer(scan_text):
            violations.append(
                Violation(
                    path=path,
                    line=line_number(scan_text, match.start()),
                    excerpt=excerpt_at(text, match.start()),
                    rule=label,
                )
            )

    violations.sort(key=lambda v: v.line)
    return violations


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


def collect_violations_from_files(files: list[Path]) -> list[Violation]:
    if not files:
        raise RuntimeError("no Rust source files found under src")

    violations: list[Violation] = []
    for path in files:
        try:
            rel = str(path.relative_to(REPO_ROOT))
        except ValueError:
            rel = str(path)
        violations.extend(find_violations_in_text(rel, production_text(path)))
    return violations


def collect_violations() -> list[Violation]:
    return collect_violations_from_files(bolt_src_files())


def main() -> int:
    violations = collect_violations()
    if violations:
        for violation in violations:
            print(
                "FAIL: UsableMu sole-mint fence: "
                f"{violation.rule} at {violation.path}:{violation.line}: {violation.excerpt}",
                file=sys.stderr,
            )
        return 1

    print("OK: UsableMu sole-mint fence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
