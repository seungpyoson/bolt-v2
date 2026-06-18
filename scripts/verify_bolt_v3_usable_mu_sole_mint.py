#!/usr/bin/env python3
"""Defense-in-depth check that `UsableMu` is minted only by the μ health gate.

THE COMPILER IS THE GUARANTEE, not this fence. `UsableMu`
(`src/bolt_v3_maker_mu_estimator.rs`) has a private field and a **module-private**
`fn new` (not `pub(crate)`). The only in-crate caller of `new` is `mint_usable_mu`
in that same module, which runs the fail-closed `evaluate_mu_health` gate over the
raw inputs before constructing — so the ONLY way to obtain a `UsableMu` in
production is to pass real inputs that clear the health check. A mint in any other
module — `UsableMu::new`, UFCS `<UsableMu>::new`, an aliased/renamed call, or a
macro expansion — is a hard compile error (E0603), because module privacy is
enforced by the compiler regardless of call syntax. The build/clippy CI lanes are
the real enforcer; that is why this fence does NOT chase UFCS/macro/alias
completeness (a text matcher provably cannot be complete against macro expansion).

This fence stays as early-warning defense-in-depth and to keep the ONE named
crate-visible seam honest:
  1. A literal `UsableMu::new` mint (call, `.map` function-reference, raw-ident
     `r#new`) anywhere in production `src/` is flagged, EXCEPT inside the body of
     `mint_usable_mu` in the seam file (the sole legitimate mint) — span-scoped via
     brace matching, not a whole-file exemption, so a rogue mint elsewhere in that
     file still fails. (The compiler already rejects mints in other modules; this
     catches a same-file regression early.)
  2. Any production `use …UsableMu as <Alias>;` import-rename or
     `type <X> = UsableMu;` type alias is flagged (no legitimate production reason
     to rename the gated newtype), plus mints through the captured alias.
  3. `new` must stay the only constructor: a `From`/`Default`/`Deserialize` impl
     for `UsableMu` (a structural mint surface) is flagged.

`#[cfg(test)]` items are stripped before scanning (shared `production_text`
helper), so the `#[cfg(test)] for_test` constructor and unit-test mints stay legal
without a production bypass constructor.
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

# `mint_usable_mu` is the sole legitimate mint of `UsableMu`: it lives in the same
# module as the (module-private) `fn new`, runs the `MuHealthReason` gate over the
# raw inputs, and only then constructs. It is the one named crate-visible seam; the
# compiler already forbids `UsableMu::new` in every other module. Every other mint
# in this seam file is a same-file regression bypassing that gate.
GATE_PATH = "src/bolt_v3_maker_mu_estimator.rs"
GATE_FN = "mint_usable_mu"


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

    # Rule 1: the gate seam function is the sole mint of `UsableMu::new`. In the
    # seam file the exemption is scoped to the `mint_usable_mu` body span; a mint
    # elsewhere in that file is a (same-file) regression. Renamed mints (Rule 2)
    # are matched via the alias set below.
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
                    rule=f"UsableMu mint outside the {GATE_FN} gate function",
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
