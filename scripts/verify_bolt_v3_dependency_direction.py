#!/usr/bin/env python3
"""Verify Bolt-v3 one-way dependency direction.

The decomposition architecture contract
(`specs/522-decompose-strategy-monolith/architecture-contract.md` §2) requires a
one-way dependency: the strategy may use shared (`bolt_v3_*`) and family
(`bolt_v3_market_families/*`) modules, but those shared/family modules must NEVER
import the strategy layer (`crate::strategies`). The three existing fences
(naming, provider-leaks, core-boundary) do not catch a shared module doing
`use crate::strategies::...`; this fence does.

To avoid being defeated by idiomatic Rust spellings, the fence parses each `use`
statement (including multi-line and grouped/nested imports), expands the
use-tree into individual paths, and resolves `crate::`, `self::`, and `super::`
roots to absolute module paths before checking whether the path enters
`crate::strategies`. So all of these are caught:

    use crate::strategies::registry::FeeProvider;
    use crate::{strategies::registry::FeeProvider, foo::Bar};
    use crate::{
        strategies::registry::FeeProvider,
        foo::Bar,
    };
    use super::strategies::registry::FeeProvider;   // from a top-level bolt_v3_* module

Each violation is keyed by (file, the absolute strategy path it imports) — NOT by
the surrounding `use` block — so the allowlist is stable against unrelated edits
to a grouped import and so a newly-added strategy symbol is caught even inside an
already-coupled `use` block.

Current code already contains pre-existing back-references (tracked under #446 and
the #522 decomposition). They are captured in `FINDING_ALLOWANCES` so the fence is
GREEN on today's code while FAILING on every NEW back-reference. The allowlist may
only SHRINK:

- adding a new allowance is forbidden — a new back-reference is a bug to fix, not
  to allow;
- a stale allowance (one that no longer matches any import) FAILS, forcing its
  removal once the underlying reference is relocated to a shared module.

The fence checks the import path, which is the mechanism a shared module would use
to reach the strategy. Like the sibling fences, it does not resolve aliased
re-exports that launder a strategy type through a third module under a new name.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
MAX_SCAN_FILE_BYTES = 1024 * 1024

# Shared/family layer = everything under `src/bolt_v3_*` (top-level files and
# their subdirectories, including `bolt_v3_market_families/`). The strategy layer
# (`src/strategies/**`) is intentionally NOT scanned — it MAY use shared/family.
SCAN_PREFIX = "src/bolt_v3_"

# The forbidden destination: the crate-root `strategies` module.
STRATEGY_ROOT = "strategies"

# A `use ...;` statement (spans newlines; `;` cannot appear inside a use path).
USE_STATEMENT = re.compile(r"\buse\b[^;]*;", re.DOTALL)

MESSAGE = (
    "shared/family module imports the strategy layer (crate::strategies); "
    "violates one-way dependency (contract §2)"
)


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    strategy_path: str

    def render(self, prefix: str) -> str:
        return f"{prefix}: {self.path}:{self.line}: {MESSAGE}: imports {self.strategy_path}"


@dataclass(frozen=True)
class FindingAllowance:
    """A FROZEN pre-existing back-reference, keyed by file + absolute strategy path.

    The allowlist may only shrink — never add entries.
    """

    path: str
    strategy_path: str


class PolicyError(Exception):
    """Raised when the dependency-direction policy cannot be evaluated safely."""


# Pre-existing strategy back-references in the shared/family layer, frozen at the
# start of the #522 decomposition (generated from the verifier's own output on
# origin/main). Each entry is removed when its underlying reference is relocated
# to a shared module. DO NOT ADD ENTRIES — a new back-reference is a bug to fix.
#
# All of these resolve under the #522 decomposition + #446 (relocate the shared
# FeeProvider trait and the strategy-owned entry-decision evidence types out of
# `strategies::` into shared modules).
FINDING_ALLOWANCES: tuple[FindingAllowance, ...] = (
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder"),
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::binary_oracle_edge_taker::KEY"),
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::production_strategy_registry"),
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::registry::StrategyBuildContext"),
    FindingAllowance("src/bolt_v3_archetypes/binary_oracle_edge_taker.rs", "strategies::registry::StrategyBuilder"),
    FindingAllowance("src/bolt_v3_canary_proof_executor.rs", "strategies::registry::FeeProvider"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryBookSideSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryBooksSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryDecisionEvidenceSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryFeeSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryRealizedVolatilitySource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleEntryReferenceQuoteSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::BinaryOracleReferenceQuoteObservationSource"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::ENTRY_DECISION_EVIDENCE_SOURCE_RECORD_KIND"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::ENTRY_DECISION_EVIDENCE_SOURCE_SCHEMA_VERSION"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::derive_entry_reference_proofs_from_quote_observations"),
    FindingAllowance("src/bolt_v3_operator_artifacts.rs", "strategies::binary_oracle_edge_taker::record_entry_decision_evidence_from_source"),
    FindingAllowance("src/bolt_v3_providers/mod.rs", "strategies::registry::FeeProvider"),
    FindingAllowance("src/bolt_v3_providers/polymarket.rs", "strategies::registry::FeeProvider"),
    FindingAllowance("src/bolt_v3_providers/polymarket/fees.rs", "strategies::registry::FeeProvider"),
)


def strip_comments(text: str) -> str:
    """Remove `//` line comments and `/* */` block comments while preserving every
    newline (so line numbers stay accurate). Comment characters become spaces."""

    out: list[str] = []
    i = 0
    n = len(text)
    in_line = False
    in_block = False
    in_string = False
    string_quote = ""
    while i < n:
        ch = text[i]
        two = text[i : i + 2]
        if in_line:
            out.append("\n" if ch == "\n" else " ")
            if ch == "\n":
                in_line = False
            i += 1
        elif in_block:
            if two == "*/":
                in_block = False
                out.append("  ")
                i += 2
            else:
                out.append("\n" if ch == "\n" else " ")
                i += 1
        elif in_string:
            out.append(ch)
            if ch == "\\" and i + 1 < n:
                out.append(text[i + 1])
                i += 2
                continue
            if ch == string_quote:
                in_string = False
            i += 1
        elif two == "//":
            in_line = True
            out.append("  ")
            i += 2
        elif two == "/*":
            in_block = True
            out.append("  ")
            i += 2
        elif ch in ('"', "'"):
            in_string = True
            string_quote = ch
            out.append(ch)
            i += 1
        else:
            out.append(ch)
            i += 1
    return "".join(out)


def _split_top_commas(inner: str) -> list[str]:
    parts: list[str] = []
    depth = 0
    start = 0
    for i, ch in enumerate(inner):
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        elif ch == "," and depth == 0:
            parts.append(inner[start:i])
            start = i + 1
    parts.append(inner[start:])
    return [p for p in (part.strip() for part in parts) if p]


def expand_use_tree(body: str) -> list[str]:
    """Expand a (whitespace-free) use-tree body into flat `::`-joined paths."""

    brace_start = body.find("{")
    if brace_start == -1:
        return [body] if body else []

    depth = 0
    brace_end = -1
    for i in range(brace_start, len(body)):
        if body[i] == "{":
            depth += 1
        elif body[i] == "}":
            depth -= 1
            if depth == 0:
                brace_end = i
                break
    if brace_end == -1:
        return [body]  # malformed; treat literally

    prefix = body[:brace_start]
    inner = body[brace_start + 1 : brace_end]
    suffix = body[brace_end + 1 :]
    results: list[str] = []
    for member in _split_top_commas(inner):
        results.extend(expand_use_tree(prefix + member + suffix))
    return results


def module_parts_for(rel: str) -> list[str]:
    """Module path components (under the crate root) for a `src/...rs` file."""

    stem = rel[len("src/") : -len(".rs")]
    parts = stem.split("/")
    if parts and parts[-1] == "mod":
        parts = parts[:-1]
    return parts


def resolve_to_absolute(segments: list[str], module_parts: list[str]) -> list[str] | None:
    """Resolve a use path's segments to absolute crate-rooted components.

    Returns None for external-crate paths (which cannot reach the local strategy
    layer) or unresolvable relative paths.
    """

    if not segments:
        return None
    head = segments[0]
    if head == "crate":
        return segments[1:]
    if head == "self":
        return module_parts + segments[1:]
    if head == "super":
        ups = 0
        while ups < len(segments) and segments[ups] == "super":
            ups += 1
        if ups > len(module_parts):
            return None
        base = module_parts[: len(module_parts) - ups]
        return base + segments[ups:]
    # Bare root (e.g. `std`, an external crate) — not the local strategy layer.
    return None


def resolved_strategy_path(path: str, module_parts: list[str]) -> str | None:
    """If `path` enters the local strategy layer, return its absolute `::`-joined
    path (e.g. ``strategies::registry::FeeProvider``); otherwise None."""

    segments = [seg for seg in path.split("::") if seg and seg != "self"]
    absolute = resolve_to_absolute(segments, module_parts)
    if absolute and absolute[0] == STRATEGY_ROOT:
        return "::".join(absolute)
    return None


def scan_files(root: Path) -> tuple[Path, ...]:
    src = root / "src"
    if not src.exists():
        return ()
    return tuple(
        sorted(
            path
            for path in src.rglob("*.rs")
            if path.is_file()
            and path.relative_to(root).as_posix().startswith(SCAN_PREFIX)
        )
    )


def read_policy_source(path: Path, rel: str) -> str:
    try:
        if path.stat().st_size > MAX_SCAN_FILE_BYTES:
            raise PolicyError(f"{rel} exceeds 1 MiB limit")
        return path.read_text(encoding="utf-8")
    except PolicyError:
        raise
    except OSError as error:
        raise PolicyError(f"failed to read {rel}: {error}") from error
    except UnicodeDecodeError as error:
        raise PolicyError(f"failed to decode {rel}: {error}") from error


def find_violations(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for path in scan_files(root):
        rel = path.relative_to(root).as_posix()
        module_parts = module_parts_for(rel)
        text = read_policy_source(path, rel)
        clean = strip_comments(text)
        for match in USE_STATEMENT.finditer(clean):
            stmt = match.group(0)
            line = clean.count("\n", 0, match.start()) + 1
            body = stmt[len("use") : -1]  # drop the `use` keyword and trailing `;`
            body = re.sub(r"\s+as\s+\w+", "", body)
            body = re.sub(r"\s+", "", body)
            seen: set[str] = set()
            for flat in expand_use_tree(body):
                strategy_path = resolved_strategy_path(flat, module_parts)
                if strategy_path is not None and strategy_path not in seen:
                    seen.add(strategy_path)
                    findings.append(
                        Finding(path=rel, line=line, strategy_path=strategy_path)
                    )
    return findings


def is_allowed(finding: Finding) -> bool:
    return any(
        allowance.path == finding.path
        and allowance.strategy_path == finding.strategy_path
        for allowance in FINDING_ALLOWANCES
    )


def main() -> int:
    try:
        findings = find_violations(REPO_ROOT)
    except PolicyError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1

    matched: set[tuple[str, str]] = set()
    real: list[Finding] = []
    for finding in findings:
        if is_allowed(finding):
            matched.add((finding.path, finding.strategy_path))
        else:
            real.append(finding)

    stale = [
        allowance
        for allowance in FINDING_ALLOWANCES
        if (allowance.path, allowance.strategy_path) not in matched
    ]

    failed = False
    for finding in real:
        print(finding.render("FAIL"), file=sys.stderr)
        failed = True
    for allowance in stale:
        print(
            f"FAIL: {allowance.path}: stale allowance no longer matches any import; "
            f"remove it (allowlist may only shrink): imports {allowance.strategy_path}",
            file=sys.stderr,
        )
        failed = True

    if failed:
        return 1

    print(
        "OK: Bolt-v3 dependency-direction verifier passed "
        f"({len(FINDING_ALLOWANCES)} frozen pre-existing back-reference(s))."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
