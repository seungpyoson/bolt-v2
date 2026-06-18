#!/usr/bin/env python3
"""Verify `UsableMu` is minted only by the μ health gate.

`UsableMu` (`src/bolt_v3_maker_mu_estimator.rs`) is the newtype that carries a
gate-cleared informed-fraction μ into the maker quote planner. Its private field
and `pub(crate) fn new` already make an *accidental* bare-`f64`
`MakerQuotePlanInputs { informed_fraction: 0.5, .. }` a hard compile error. But a
`pub(crate)` constructor leaves a *deliberate* in-crate `UsableMu::new(raw_f64)`
outside the gate possible, which would route an ungated μ around the
`MuHealthReason` checks (the exact class MU-3 closes: "an ungated μ reaches the
planner"). This fence makes the sole-mint property structural, not convention:
the only production mint of a `UsableMu` is the gate
`MakerMuState::usable_mu_for` in `src/strategies/binary_oracle_maker/mu.rs`.

A `UsableMu::new` reference (call `UsableMu::new(...)` or function-reference
`.map(UsableMu::new)`) in any production Rust source under `src/` other than the
gate file fails the fence. `#[cfg(test)]` items are stripped before scanning
(via the shared `production_text` helper), so unit-test mints stay legal without
a public bypass constructor.
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

# Matches both the call form `UsableMu::new(...)` and the function-reference form
# `UsableMu::new` (e.g. `.map(UsableMu::new)`); the trailing negative lookahead
# prevents matching a longer identifier such as `UsableMu::new_unchecked`.
USABLE_MU_NEW = re.compile(r"(?<![A-Za-z0-9_])UsableMu::(?:r#)?new(?![A-Za-z0-9_])")

# The μ health gate is the sole legitimate mint of `UsableMu`: `usable_mu_for`
# clears the `MuHealthReason` checks and only then maps the cleared value through
# `UsableMu::new`. Every other production mint is a bypass of that gate.
GATE_PATH = "src/strategies/binary_oracle_maker/mu.rs"


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    excerpt: str


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    if path == GATE_PATH:
        return []
    scan_text = strip_rust_comments_and_literals(text)
    violations: list[Violation] = []
    for match in USABLE_MU_NEW.finditer(scan_text):
        line_start = text.rfind("\n", 0, match.start()) + 1
        line_end = text.find("\n", match.end())
        if line_end == -1:
            line_end = len(text)
        violations.append(
            Violation(
                path=path,
                line=line_number(scan_text, match.start()),
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
                "FAIL: UsableMu sole-mint fence: production mint outside the μ gate "
                f"({GATE_PATH}) at {violation.path}:{violation.line}: {violation.excerpt}",
                file=sys.stderr,
            )
        return 1

    print("OK: UsableMu sole-mint fence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
