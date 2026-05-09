#!/usr/bin/env python3
"""Report Bolt-v3 nucleus admission blockers.

This is a repository-governance audit, not a production runtime path. Default
mode reports blockers and exits successfully; `--strict` exits nonzero when the
nucleus is blocked or unscannable.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import importlib.util
import re
import sys
import textwrap
from dataclasses import dataclass, field
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent

BLOCKER_CLASS_ORDER = {
    "generic-contract-leak": 0,
    "missing-contract-surface": 1,
    "unowned-runtime-default": 2,
    "unfenced-concrete-fixture": 3,
    "narrow-verifier-bypass": 4,
    "invalid-waiver": 5,
    "scan-universe-unproven": 6,
}

CONCRETE_TOKENS = (
    "polymarket",
    "binance",
    "updown",
    "BTC",
    "BTCUSDT",
    "Chainlink",
    "binary_oracle_edge_taker",
)

GENERIC_CONTRACT_LEAK_PATTERNS = tuple(
    re.compile(pattern)
    for pattern in (
        r"\bMarketIdentityPlan\b",
        r"\bBoltV3UpdownNowFn\b",
        r"\bbinary_oracle_edge_taker::KEY\b",
        r"\bupdown::KEY\b",
    )
)

_PROVIDER_LEAKS_MODULES: dict[Path, object | None] = {}
_PROVIDER_LEAKS_LOAD_ERRORS: dict[Path, str] = {}

SECRET_ASSIGNMENT = re.compile(
    r"(?i)([\"']?\b(?:api[_-]?key|api[_-]?secret|private[_-]?key|passphrase|token)\b[\"']?)"
    r"(\s*[:=]\s*)"
    r"(\"[^\"]*\"|'[^']*'|[^,\s#}]+)"
)


@dataclass(frozen=True)
class EvidenceRecord:
    path: str
    line: int | None = None
    excerpt: str | None = None
    absence_query: str | None = None
    explanation: str = ""

    def render(self) -> str:
        if self.absence_query is not None:
            return f"  - absent :: {self.absence_query}"
        location = self.path if self.line is None else f"{self.path}:{self.line}"
        excerpt = self.excerpt or ""
        return f"  - {location} :: {excerpt}"


@dataclass(frozen=True)
class AdmissionBlocker:
    blocker_id: str
    blocker_class: str
    invariant_id: str
    evidence: tuple[EvidenceRecord, ...]
    retire_when: str
    severity: str = "blocker"

    def render(self) -> str:
        evidence = "\n".join(record.render() for record in self.evidence)
        return "\n".join(
            [
                f"BLOCKER {self.blocker_id}",
                f"class: {self.blocker_class}",
                f"invariant: {self.invariant_id}",
                f"severity: {self.severity}",
                "evidence:",
                evidence,
                f"retire_when: {self.retire_when}",
            ]
        )


@dataclass(frozen=True)
class Waiver:
    blocker_id: str
    path: str
    excerpt: str
    rationale: str
    retirement_issue: str

    def missing_fields(self) -> tuple[str, ...]:
        missing = []
        for field_name in (
            "blocker_id",
            "path",
            "excerpt",
            "rationale",
            "retirement_issue",
        ):
            if not getattr(self, field_name):
                missing.append(field_name)
        return tuple(missing)


@dataclass(frozen=True)
class ScanUniverse:
    files: tuple[Path, ...]
    skipped: tuple[str, ...] = ()
    v3_source_files: tuple[Path, ...] = ()

    @property
    def file_count(self) -> int:
        return len(self.files)


@dataclass(frozen=True)
class AdmissionAuditRun:
    mode: str
    repo_root: Path
    scan_universe: ScanUniverse
    blockers: tuple[AdmissionBlocker, ...] = field(default_factory=tuple)
    warnings: tuple[str, ...] = field(default_factory=tuple)

    @property
    def verdict(self) -> str:
        if any(blocker.blocker_class == "scan-universe-unproven" for blocker in self.blockers):
            return "UNSCANNABLE"
        if self.blockers:
            return "BLOCKED"
        return "ADMITTED"

    def render(self) -> str:
        lines = [
            f"mode: {self.mode}",
            f"VERDICT: {self.verdict}",
            (
                "scan-universe: "
                f"files={self.scan_universe.file_count} "
                f"v3_source={len(self.scan_universe.v3_source_files)} "
                f"skipped={len(self.scan_universe.skipped)}"
            ),
        ]
        if self.scan_universe.skipped:
            lines.append("skipped:")
            lines.extend(f"  - {item}" for item in self.scan_universe.skipped)
        if self.blockers:
            lines.append("blockers:")
            lines.extend(blocker.render() for blocker in sorted_blockers(self.blockers))
        if self.warnings:
            lines.append("warnings:")
            lines.extend(f"  - {warning}" for warning in self.warnings)
        return "\n".join(lines)


def safe_excerpt(line: str) -> str:
    normalized = " ".join(line.strip().split())
    return SECRET_ASSIGNMENT.sub(lambda match: f"{match.group(1)}{match.group(2)}<redacted>", normalized)


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def provider_leaks_module(root: Path):
    root = root.resolve()
    if root in _PROVIDER_LEAKS_MODULES:
        return _PROVIDER_LEAKS_MODULES[root]
    _PROVIDER_LEAKS_LOAD_ERRORS.pop(root, None)
    provider_leaks = root / "scripts" / "verify_bolt_v3_provider_leaks.py"
    if not provider_leaks.exists():
        _PROVIDER_LEAKS_MODULES[root] = None
        _PROVIDER_LEAKS_LOAD_ERRORS[root] = f"{rel(provider_leaks, root)} is missing"
        return None

    module_suffix = hashlib.sha256(root.as_posix().encode("utf-8")).hexdigest()[:16]
    module_name = f"_bolt_v3_provider_leaks_{module_suffix}"
    spec = importlib.util.spec_from_file_location(module_name, provider_leaks)
    if spec is None or spec.loader is None:
        _PROVIDER_LEAKS_MODULES[root] = None
        _PROVIDER_LEAKS_LOAD_ERRORS[root] = f"could not create import spec for {rel(provider_leaks, root)}"
        return None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    try:
        spec.loader.exec_module(module)
    except Exception as exc:
        sys.modules.pop(spec.name, None)
        _PROVIDER_LEAKS_MODULES[root] = None
        _PROVIDER_LEAKS_LOAD_ERRORS[root] = f"{rel(provider_leaks, root)} import failed: {type(exc).__name__}: {exc}"
        return None
    if not hasattr(module, "production_text"):
        sys.modules.pop(spec.name, None)
        _PROVIDER_LEAKS_MODULES[root] = None
        _PROVIDER_LEAKS_LOAD_ERRORS[root] = f"{rel(provider_leaks, root)} has no production_text()"
        return None
    _PROVIDER_LEAKS_MODULES[root] = module
    return module


def blank_preserving_lines(text: str) -> str:
    return "\n" * text.count("\n")


def production_text(root: Path, path: Path, text: str) -> str:
    """Return a Rust production scan view, or fail closed if the helper is unavailable."""
    if path.suffix != ".rs":
        return text
    module = provider_leaks_module(root)
    if module is None:
        return blank_preserving_lines(text)
    return module.production_text(text)


def rel(path: Path, root: Path) -> str:
    return path.relative_to(root).as_posix()


def discover_scan_universe(root: Path) -> ScanUniverse:
    candidates: set[Path] = set()

    src = root / "src"
    if src.exists():
        candidates.update(src.glob("bolt_v3*.rs"))
        for directory in src.glob("bolt_v3*"):
            if directory.is_dir():
                candidates.update(directory.rglob("*.rs"))

    tests = root / "tests"
    if tests.exists():
        candidates.update(tests.glob("bolt_v3*.rs"))
        for directory in tests.glob("bolt_v3*"):
            if directory.is_dir():
                candidates.update(directory.rglob("*.rs"))
        fixtures = tests / "fixtures"
        if fixtures.exists():
            candidates.update(path for path in fixtures.rglob("*") if path.is_file())

    for docs_root in (root / "docs" / "bolt-v3", root / "specs" / "001-v3-nucleus-admission"):
        if docs_root.exists():
            candidates.update(path for path in docs_root.rglob("*") if path.is_file())

    scripts = root / "scripts"
    if scripts.exists():
        candidates.update(scripts.glob("verify_bolt_v3*.py"))

    for path in (root / "justfile",):
        if path.exists():
            candidates.add(path)

    workflow_root = root / ".github" / "workflows"
    if workflow_root.exists():
        candidates.update(
            path
            for path in workflow_root.rglob("*")
            if path.is_file() and path.suffix in {".yml", ".yaml"}
        )

    readable: list[Path] = []
    skipped: list[str] = []
    for path in sorted(candidates):
        try:
            path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            skipped.append(f"{rel(path, root)}: non-UTF-8")
            continue
        except OSError as exc:
            skipped.append(f"{rel(path, root)}: {exc}")
            continue
        readable.append(path)

    v3_source_files = tuple(
        path
        for path in readable
        if rel(path, root).startswith("src/bolt_v3") and path.suffix == ".rs"
    )
    return ScanUniverse(tuple(readable), tuple(skipped), v3_source_files)


def is_generic_core_path(relative_path: str) -> bool:
    if relative_path.startswith("src/bolt_v3_providers/"):
        return relative_path == "src/bolt_v3_providers/mod.rs"
    if relative_path.startswith("src/bolt_v3_market_families/"):
        return relative_path == "src/bolt_v3_market_families/mod.rs"
    if relative_path.startswith("src/bolt_v3_archetypes/"):
        return relative_path == "src/bolt_v3_archetypes/mod.rs"
    return relative_path.startswith("src/bolt_v3") and relative_path.endswith(".rs")


def is_allowed_concrete_context(relative_path: str) -> bool:
    allowed_prefixes = (
        "tests/fixtures/bolt_v3/",
        "src/bolt_v3_providers/",
        "src/bolt_v3_market_families/",
        "src/bolt_v3_archetypes/",
        "docs/bolt-v3/",
        "specs/001-v3-nucleus-admission/",
    )
    if relative_path in (
        "src/bolt_v3_providers/mod.rs",
        "src/bolt_v3_market_families/mod.rs",
        "src/bolt_v3_archetypes/mod.rs",
    ):
        return False
    return relative_path.startswith(allowed_prefixes)


def evidence_for_match(root: Path, path: Path, text: str, pos: int) -> EvidenceRecord:
    line_start = text.rfind("\n", 0, pos) + 1
    line_end = text.find("\n", pos)
    if line_end == -1:
        line_end = len(text)
    return EvidenceRecord(
        path=rel(path, root),
        line=line_number(text, pos),
        excerpt=safe_excerpt(text[line_start:line_end]),
    )


def detect_generic_contract_leaks(root: Path, universe: ScanUniverse) -> list[AdmissionBlocker]:
    evidence: list[EvidenceRecord] = []
    for path in universe.files:
        relative_path = rel(path, root)
        if not is_generic_core_path(relative_path):
            continue
        raw = path.read_text(encoding="utf-8")
        text = production_text(root, path, raw)
        for pattern in GENERIC_CONTRACT_LEAK_PATTERNS:
            for match in pattern.finditer(text):
                evidence.append(evidence_for_match(root, path, text, match.start()))

    if not evidence:
        return []

    return [
        AdmissionBlocker(
            blocker_id="generic-contract-leak",
            blocker_class="generic-contract-leak",
            invariant_id="generic-contract-boundaries",
            evidence=tuple(sorted_evidence(evidence)),
            retire_when=(
                "generic provider/adapter/family/archetype boundaries carry no concrete "
                "market-family plan, clock, provider, family, or archetype names"
            ),
        )
    ]


def detect_missing_contract_surfaces(root: Path, universe: ScanUniverse) -> list[AdmissionBlocker]:
    searchable_parts: list[str] = []
    for path in universe.files:
        if not rel(path, root).startswith("src/bolt_v3"):
            continue
        raw = path.read_text(encoding="utf-8")
        searchable_parts.append(production_text(root, path, raw))
    searchable = "\n".join(searchable_parts)
    required = {
        "decision-event contract": (
            ("DecisionEvent",),
            ("CustomDataTrait",),
            ("ensure_custom_data_registered",),
        ),
        "conformance harness": (("conformance", "ConformanceHarness"),),
        "BacktestEngine/live parity boundary": (
            ("BacktestEngine", "BacktestEngineLiveParityBoundary"),
            ("add_strategy",),
        ),
    }

    evidence: list[EvidenceRecord] = []
    for surface, required_groups in required.items():
        if not all(any(contains_identifier(searchable, term) for term in terms) for terms in required_groups):
            terms = tuple(term for group in required_groups for term in group)
            evidence.append(
                EvidenceRecord(
                    path="src/bolt_v3*",
                    absence_query=f"searched=src/bolt_v3* terms={','.join(terms)}",
                    explanation=surface,
                )
            )

    if not evidence:
        return []

    return [
        AdmissionBlocker(
            blocker_id="missing-contract-surface",
            blocker_class="missing-contract-surface",
            invariant_id="bolt-v3-nucleus-admission-rules",
            evidence=tuple(evidence),
            retire_when=(
                "V3 defines decision-event, conformance, and BacktestEngine/live parity "
                "contract surfaces before behavior work continues"
            ),
        )
    ]


def contains_identifier(text: str, identifier: str) -> bool:
    return re.search(rf"(?<![A-Za-z0-9_]){re.escape(identifier)}(?![A-Za-z0-9_])", text) is not None


def detect_unowned_runtime_defaults(root: Path, universe: ScanUniverse) -> list[AdmissionBlocker]:
    evidence: list[EvidenceRecord] = []
    for path in universe.files:
        relative_path = rel(path, root)
        if not (relative_path.startswith("src/bolt_v3") and path.suffix == ".rs"):
            continue
        text = production_text(root, path, path.read_text(encoding="utf-8"))
        for match in re.finditer(r"\bDefault::default\(\)", text):
            evidence.append(evidence_for_match(root, path, text, match.start()))

    if not evidence:
        return []

    return [
        AdmissionBlocker(
            blocker_id="unowned-runtime-default",
            blocker_class="unowned-runtime-default",
            invariant_id="single-source-runtime-configuration",
            evidence=tuple(sorted_evidence(evidence)),
            retire_when="V3 NT runtime mappings use config/catalog-owned values instead of Default::default()",
        )
    ]


def detect_unfenced_fixture_values(root: Path, universe: ScanUniverse) -> list[AdmissionBlocker]:
    evidence: list[EvidenceRecord] = []
    token_pattern = concrete_token_pattern()
    for path in universe.files:
        relative_path = rel(path, root)
        if not has_path_segment(relative_path, "fixtures") or is_allowed_concrete_context(relative_path):
            continue
        text = path.read_text(encoding="utf-8")
        for match in token_pattern.finditer(text):
            evidence.append(evidence_for_match(root, path, text, match.start()))

    if not evidence:
        return []

    return [
        AdmissionBlocker(
            blocker_id="unfenced-concrete-fixture",
            blocker_class="unfenced-concrete-fixture",
            invariant_id="generic-contract-boundaries",
            evidence=tuple(sorted_evidence(evidence)),
            retire_when="concrete fixture values are under an explicit allowed fixture/catalog context",
        )
    ]


def concrete_token_pattern() -> re.Pattern[str]:
    return re.compile(
        "|".join(rf"(?<![A-Za-z0-9_]){re.escape(token)}(?![A-Za-z0-9_])" for token in CONCRETE_TOKENS),
        re.IGNORECASE,
    )


def has_path_segment(relative_path: str, segment: str) -> bool:
    return segment in relative_path.split("/")


def detect_narrow_verifier_bypass(root: Path, universe: ScanUniverse) -> list[AdmissionBlocker]:
    path = root / "scripts" / "verify_bolt_v3_provider_leaks.py"
    if path not in universe.files:
        return []
    text = path.read_text(encoding="utf-8")
    segment = assignment_source_segment(text, "FINDING_ALLOWANCES")
    if segment is None:
        return []
    segment_text, start_line = segment
    evidence = [
        evidence_for_segment_match(root, path, segment_text, start_line, match.start())
        for pattern in GENERIC_CONTRACT_LEAK_PATTERNS
        for match in pattern.finditer(segment_text)
    ]
    if not evidence:
        return []
    return [
        AdmissionBlocker(
            blocker_id="narrow-verifier-bypass",
            blocker_class="narrow-verifier-bypass",
            invariant_id="bolt-v3-nucleus-admission-rules",
            evidence=tuple(sorted_evidence(evidence)),
            retire_when="narrow provider-leak allowlists cannot suppress nucleus admission blockers",
        )
    ]


def assignment_source_segment(text: str, name: str) -> tuple[str, int] | None:
    try:
        tree = ast.parse(textwrap.dedent(text))
    except SyntaxError:
        return None

    lines = text.splitlines(keepends=True)
    for node in tree.body:
        if isinstance(node, ast.Assign):
            if not any(isinstance(target, ast.Name) and target.id == name for target in node.targets):
                continue
        elif isinstance(node, ast.AnnAssign):
            if not isinstance(node.target, ast.Name) or node.target.id != name:
                continue
        else:
            continue
        end_lineno = getattr(node, "end_lineno", node.lineno)
        return "".join(lines[node.lineno - 1 : end_lineno]), node.lineno
    return None


def evidence_for_segment_match(root: Path, path: Path, text: str, start_line: int, pos: int) -> EvidenceRecord:
    line_start = text.rfind("\n", 0, pos) + 1
    line_end = text.find("\n", pos)
    if line_end == -1:
        line_end = len(text)
    return EvidenceRecord(
        path=rel(path, root),
        line=start_line + line_number(text, pos) - 1,
        excerpt=safe_excerpt(text[line_start:line_end]),
    )


def scan_universe_blockers(universe: ScanUniverse) -> list[AdmissionBlocker]:
    evidence: list[EvidenceRecord] = []
    if universe.file_count == 0:
        evidence.append(
            EvidenceRecord(
                path=".",
                absence_query="searched=V3 source/tests/fixtures/docs/scripts/justfile/workflows terms=*",
            )
        )
    if not universe.v3_source_files:
        evidence.append(
            EvidenceRecord(path="src", absence_query="searched=src terms=bolt_v3*.rs,bolt_v3_*/**/*.rs")
        )
    if not evidence:
        return []
    return [
        AdmissionBlocker(
            blocker_id="scan-universe-unproven",
            blocker_class="scan-universe-unproven",
            invariant_id="evidence-first-architecture",
            evidence=tuple(evidence),
            retire_when="the audit proves which V3 source files were inspected",
        )
    ]


def provider_leaks_dependency_blocker(root: Path) -> list[AdmissionBlocker]:
    root = root.resolve()
    error = _PROVIDER_LEAKS_LOAD_ERRORS.get(root)
    if error is None:
        return []
    return [
        AdmissionBlocker(
            blocker_id="provider-leaks-production-text-unavailable",
            blocker_class="scan-universe-unproven",
            invariant_id="evidence-first-architecture",
            evidence=(
                EvidenceRecord(
                    path="scripts/verify_bolt_v3_provider_leaks.py",
                    excerpt=safe_excerpt(error),
                ),
            ),
            retire_when="the audit can import provider-leak production_text() before proving admission",
        )
    ]


def invalid_waiver_blockers(waivers: tuple[Waiver, ...]) -> list[AdmissionBlocker]:
    blockers: list[AdmissionBlocker] = []
    for index, waiver in enumerate(waivers, start=1):
        missing = waiver.missing_fields()
        if not missing:
            continue
        blockers.append(
            AdmissionBlocker(
                blocker_id=f"invalid-waiver-{index}",
                blocker_class="invalid-waiver",
                invariant_id="empirical-readiness-and-review-gates",
                evidence=(
                    EvidenceRecord(
                        path="<waiver>",
                        excerpt=f"missing={','.join(missing)} blocker_id={waiver.blocker_id or '<missing>'}",
                    ),
                ),
                retire_when="waivers include path, excerpt, blocker id, rationale, and retirement issue",
            )
        )
    return blockers


def waiver_matches_record(blocker: AdmissionBlocker, record: EvidenceRecord, waiver: Waiver) -> bool:
    if waiver.missing_fields() or waiver.blocker_id != blocker.blocker_id:
        return False
    return record.path == waiver.path and record.excerpt is not None and record.excerpt == waiver.excerpt


def apply_waivers_to_blocker(blocker: AdmissionBlocker, waivers: tuple[Waiver, ...]) -> AdmissionBlocker:
    remaining = tuple(
        record
        for record in blocker.evidence
        if not any(waiver_matches_record(blocker, record, waiver) for waiver in waivers)
    )
    return AdmissionBlocker(
        blocker_id=blocker.blocker_id,
        blocker_class=blocker.blocker_class,
        invariant_id=blocker.invariant_id,
        evidence=remaining,
        retire_when=blocker.retire_when,
        severity=blocker.severity,
    )


def apply_waivers(
    blockers: list[AdmissionBlocker] | tuple[AdmissionBlocker, ...],
    waivers: tuple[Waiver, ...],
) -> list[AdmissionBlocker]:
    waived: list[AdmissionBlocker] = []
    for blocker in blockers:
        narrowed = apply_waivers_to_blocker(blocker, waivers)
        if narrowed.evidence:
            waived.append(narrowed)
    return waived


def sorted_evidence(records: list[EvidenceRecord] | tuple[EvidenceRecord, ...]) -> list[EvidenceRecord]:
    unique = {
        (
            record.path,
            record.line,
            record.excerpt,
            record.absence_query,
        ): record
        for record in records
    }
    return sorted(
        unique.values(),
        key=lambda record: (record.path, record.line or 0, record.excerpt or "", record.absence_query or ""),
    )


def sorted_blockers(blockers: tuple[AdmissionBlocker, ...]) -> list[AdmissionBlocker]:
    return sorted(
        blockers,
        key=lambda blocker: (
            BLOCKER_CLASS_ORDER.get(blocker.blocker_class, 99),
            blocker.blocker_id,
            blocker.evidence[0].path if blocker.evidence else "",
            blocker.evidence[0].line or 0 if blocker.evidence else 0,
        ),
    )


def audit_root(root: Path, mode: str = "report", waivers: list[Waiver] | tuple[Waiver, ...] | None = None) -> AdmissionAuditRun:
    root = root.resolve()
    waiver_tuple = tuple(waivers or ())
    universe = discover_scan_universe(root)
    provider_leaks_module(root)
    blockers: list[AdmissionBlocker] = []
    blockers.extend(scan_universe_blockers(universe))
    blockers.extend(invalid_waiver_blockers(waiver_tuple))
    blockers.extend(detect_generic_contract_leaks(root, universe))
    blockers.extend(detect_missing_contract_surfaces(root, universe))
    blockers.extend(detect_unowned_runtime_defaults(root, universe))
    blockers.extend(detect_unfenced_fixture_values(root, universe))
    blockers.extend(detect_narrow_verifier_bypass(root, universe))
    blockers.extend(provider_leaks_dependency_blocker(root))

    unwaived = apply_waivers(blockers, waiver_tuple)
    return AdmissionAuditRun(
        mode=mode,
        repo_root=root,
        scan_universe=universe,
        blockers=tuple(sorted_blockers(tuple(unwaived))),
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="repository root to scan; defaults to this checkout",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="exit nonzero when blockers are present",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    mode = "strict" if args.strict else "report"
    run = audit_root(args.repo_root, mode=mode)
    print(run.render())
    return 1 if args.strict and run.verdict != "ADMITTED" else 0


if __name__ == "__main__":
    raise SystemExit(main())
