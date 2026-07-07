#!/usr/bin/env python3
"""Verify Bolt-v3 production surfaces do not reach legacy default paths."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

from verify_bolt_v3_pure_rust_runtime import production_text
from verifier_io import require_nonempty


REPO_ROOT = Path(__file__).resolve().parent.parent


def _runtime_source_paths() -> tuple[str, ...]:
    paths = {
        "src/main.rs",
        "src/lake_batch.rs",
        "src/log_sweep.rs",
        "src/raw_types.rs",
        "src/secrets.rs",
        "src/venue_contract.rs",
        *(
            # rglob (not glob) so the strategy module's submodule files are
            # scanned after the A3/A8 split moved them into the strategy
            # directory module.
            # A non-recursive glob would silently drop the strategy production
            # source and weaken this fence.
            path.relative_to(REPO_ROOT).as_posix()
            for path in (REPO_ROOT / "src" / "strategies").rglob("*.rs")
            if path.is_file()
        ),
        *(
            path.relative_to(REPO_ROOT).as_posix()
            for path in (REPO_ROOT / "src").glob("bolt_v3*.rs")
            if path.is_file()
        ),
        *(
            path.relative_to(REPO_ROOT).as_posix()
            for directory in (REPO_ROOT / "src").glob("bolt_v3_*")
            if directory.is_dir()
            for path in directory.rglob("*.rs")
        ),
    }
    return tuple(sorted(paths))


RUNTIME_SOURCE_PATHS = _runtime_source_paths()

FORBIDDEN_REFERENCES = (
    (
        re.compile(r"\bcrate::config\b|\bbolt_v2::config\b"),
        "legacy config module",
    ),
    (
        re.compile(r"\bcrate::live_config\b|\bbolt_v2::live_config\b"),
        "legacy live_config module",
    ),
    (
        re.compile(r"\bclients::polymarket\b|\bcrate::clients::polymarket\b"),
        "legacy Polymarket client module",
    ),
    (
        re.compile(r"\bclients::chainlink\b|\bcrate::clients::chainlink\b"),
        "legacy Chainlink client module",
    ),
    (
        re.compile(
            r"\bplatform::polymarket_catalog\b"
            r"|\bcrate::platform::polymarket_catalog\b"
            r"|\bpolymarket_catalog::"
        ),
        "legacy Polymarket catalog defaults",
    ),
    (
        re.compile(r"\bConfig::load\b"),
        "legacy Config::load path",
    ),
    (
        re.compile(r"\bLiveLocalConfig\b|\bRuntimeConfig::load\b|\bmaterialize_live_config\b"),
        "legacy live-local materialization path",
    ),
)

# Scope of the Default fence: it catches every SINGLE-LINE hand-written Default
# form — derive/serde/enum attributes, `impl [const] [path::]Default for`,
# `Default::default(`, and `Type::default(` (including spaced `::` and a leading
# `::` path). MULTILINE attribute/impl formatting is not matched line-by-line here,
# but the mandatory `cargo fmt --check` CI gate normalizes such forms onto a single
# line (where these patterns then trip), so a multiline bypass cannot survive a
# formatted tree. MACRO-GENERATED `Default` impls are an inherent limitation of any
# text fence and are out of scope.
FORBIDDEN_DEFAULTS = (
    (
        re.compile(r"#\s*\[\s*derive\s*\([^\)]*\bDefault\b[^\)]*\)\s*\]"),
        "production derive Default",
    ),
    (
        re.compile(
            r"\bimpl\b(?:\s*<[^>]*>)?\s+(?:const\s+)?(?:::\s*)?"
            r"(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*Default\s+for\b"
        ),
        "production impl Default",
    ),
    (
        re.compile(r"#\s*\[\s*default\s*\]"),
        "production enum default",
    ),
    (
        re.compile(r"#\s*\[\s*serde\s*\([^\)]*\bdefault\b[^\)]*\)\s*\]"),
        "production serde default",
    ),
    (
        re.compile(r"\bDefault\s*::\s*default\s*\("),
        "production Default::default",
    ),
    (
        re.compile(
            r"\b(?!Default\b)[A-Za-z_][A-Za-z0-9_]*"
            r"(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*\s*::\s*default\s*\("
        ),
        "production type default",
    ),
    (
        re.compile(r"\.\s*or_default\s*\("),
        "production or_default",
    ),
    (
        re.compile(r"\.\s*unwrap_or_default\s*\("),
        "production unwrap_or_default",
    ),
)


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    label: str
    text: str


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    violations: list[Violation] = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        for pattern, label in FORBIDDEN_REFERENCES:
            if pattern.search(line):
                violations.append(
                    Violation(
                        path=path,
                        line=line_number,
                        label=label,
                        text=line.strip(),
                    )
                )
        for pattern, label in FORBIDDEN_DEFAULTS:
            if is_allowed_default_reference(path, line):
                continue
            if pattern.search(line):
                violations.append(
                    Violation(
                        path=path,
                        line=line_number,
                        label=label,
                        text=line.strip(),
                    )
                )
    return violations


def is_allowed_default_reference(path: str, line: str) -> bool:
    allowed_validate_defaults = {
        "src/bolt_v3_validate/nt_blocks.rs": (
            "nautilus_live::config::LiveDataEngineConfig::default()",
            "nautilus_live::config::LiveExecEngineConfig::default()",
        ),
        "src/bolt_v3_validate/risk.rs": (
            "nautilus_live::config::LiveRiskEngineConfig::default()",
        ),
    }
    return any(marker in line for marker in allowed_validate_defaults.get(path, ()))


def collect_violations(paths: tuple[str, ...] = RUNTIME_SOURCE_PATHS) -> list[Violation]:
    floor_errors: list[str] = []
    if not require_nonempty(paths, "runtime source paths", floor_errors):
        return [
            Violation(
                path=".",
                line=0,
                label=error,
                text="",
            )
            for error in floor_errors
        ]
    violations: list[Violation] = []
    for relative_path in paths:
        path = REPO_ROOT / relative_path
        if not path.is_file():
            violations.append(
                Violation(
                    path=relative_path,
                    line=0,
                    label="missing expected runtime source",
                    text="",
                )
            )
            continue
        violations.extend(find_violations_in_text(relative_path, production_text(path)))
    return violations


def main() -> int:
    violations = collect_violations()
    if violations:
        for violation in violations:
            print(
                "FAIL: Bolt-v3 production surface reaches "
                f"{violation.label} at {violation.path}:{violation.line}: {violation.text}",
                file=sys.stderr,
            )
        return 1

    print("OK: Bolt-v3 legacy default fence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
