#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 NT-owned naming verifier."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import sys
import tempfile
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_naming.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_naming", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


AUDIT_TEXT = """
audit_id: "probe"
version: 1
rules:
  - name: "fixture"
    include_globs:
      - "src/**/*.rs"
renamed_in_current_audit:
  - from: "VenueKind"
    to: "ProviderKey"
defensive_forbidden:
  - from: "StrategyArchetype"
    to: "StrategyArchetypeKey"
path_scoped_forbidden:
  - from: "MarketSlugFilter"
    to: "ProviderOwnedFilter"
    include_globs:
      - "src/core.rs"
    reason: "provider boundary"
accepted_non_nt_names: []
""".lstrip()


def test_load_audit_parses_repo_local_yaml_subset() -> None:
    original_audit_path = VERIFIER.AUDIT_PATH
    with tempfile.TemporaryDirectory() as tmp:
        audit_path = Path(tmp) / "audit.yaml"
        audit_path.write_text(AUDIT_TEXT, encoding="utf-8")
        try:
            VERIFIER.AUDIT_PATH = audit_path
            audit = VERIFIER.load_audit()
        finally:
            VERIFIER.AUDIT_PATH = original_audit_path

    scoped = audit["path_scoped_forbidden"][0]
    if scoped["include_globs"] != ["src/core.rs"] or scoped["reason"] != "provider boundary":
        raise AssertionError(f"nested list parse failed: {scoped!r}")


def test_load_audit_handles_inline_comments_and_single_quotes() -> None:
    original_audit_path = VERIFIER.AUDIT_PATH
    audit_text = """
audit_id: 'probe' # inline comment
version: 1
rules:
  - name: 'fixture' # rule comment
    include_globs:
      - 'src/**/*.rs' # glob comment
renamed_in_current_audit: []
defensive_forbidden: []
path_scoped_forbidden: []
accepted_non_nt_names: []
""".lstrip()
    with tempfile.TemporaryDirectory() as tmp:
        audit_path = Path(tmp) / "audit.yaml"
        audit_path.write_text(audit_text, encoding="utf-8")
        try:
            VERIFIER.AUDIT_PATH = audit_path
            audit = VERIFIER.load_audit()
        finally:
            VERIFIER.AUDIT_PATH = original_audit_path

    if audit["audit_id"] != "probe":
        raise AssertionError(f"single-quoted scalar parse failed: {audit!r}")
    rule = audit["rules"][0]
    if rule["name"] != "fixture" or rule["include_globs"] != ["src/**/*.rs"]:
        raise AssertionError(f"inline comment parse failed: {rule!r}")


def test_load_audit_rejects_unsupported_yaml_subset() -> None:
    original_audit_path = VERIFIER.AUDIT_PATH
    cases = {
        "missing-colon": "audit_id \"probe\"\n",
        "block-scalar": "audit_id: |\n  probe\n",
    }
    try:
        with tempfile.TemporaryDirectory() as tmp:
            for name, audit_text in cases.items():
                audit_path = Path(tmp) / f"{name}.yaml"
                audit_path.write_text(audit_text, encoding="utf-8")
                VERIFIER.AUDIT_PATH = audit_path
                try:
                    VERIFIER.load_audit()
                except ValueError:
                    continue
                raise AssertionError(f"expected ValueError for {name}")
    finally:
        VERIFIER.AUDIT_PATH = original_audit_path


def test_word_regex_is_bounded_to_identifier_words() -> None:
    regex = VERIFIER.word_re("VenueKind")
    if not regex.search("VenueKind::Polymarket"):
        raise AssertionError("expected exact identifier match")
    if regex.search("LegacyVenueKindName"):
        raise AssertionError("unexpected subword match")


def test_scan_paths_excludes_audit_target_git_and_reviews() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_scan_globs = VERIFIER.SCAN_GLOBS
    original_excluded = VERIFIER.EXCLUDED_RELATIVE_PATHS
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for rel in [
            "src/core.rs",
            "target/generated.rs",
            ".git/config.rs",
            "reviews/review.rs",
            "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml",
        ]:
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("probe\n", encoding="utf-8")
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.SCAN_GLOBS = ["**/*.rs", "docs/**/*.yaml"]
            VERIFIER.EXCLUDED_RELATIVE_PATHS = {
                "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml",
            }
            paths = {path.relative_to(root).as_posix() for path in VERIFIER.scan_paths()}
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.SCAN_GLOBS = original_scan_globs
            VERIFIER.EXCLUDED_RELATIVE_PATHS = original_excluded

    if paths != {"src/core.rs"}:
        raise AssertionError(f"unexpected scanned paths: {sorted(paths)}")


def test_main_reports_forbidden_and_required_names() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    original_docs = VERIFIER.CANONICAL_DOCS
    original_scan_globs = VERIFIER.SCAN_GLOBS
    original_excluded = VERIFIER.EXCLUDED_RELATIVE_PATHS
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "audit.yaml"
        audit_path.write_text(AUDIT_TEXT, encoding="utf-8")
        source = root / "src" / "core.rs"
        source.parent.mkdir(parents=True)
        source.write_text("pub type X = VenueKind;\n", encoding="utf-8")
        docs = root / "docs" / "contract.md"
        docs.parent.mkdir(parents=True)
        docs.write_text("ProviderKey\n", encoding="utf-8")
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            VERIFIER.CANONICAL_DOCS = [docs]
            VERIFIER.SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.EXCLUDED_RELATIVE_PATHS = set()
            with contextlib.redirect_stderr(stderr):
                code = VERIFIER.main()
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path
            VERIFIER.CANONICAL_DOCS = original_docs
            VERIFIER.SCAN_GLOBS = original_scan_globs
            VERIFIER.EXCLUDED_RELATIVE_PATHS = original_excluded

    output = stderr.getvalue()
    if code != 1 or "forbidden 'VenueKind'" not in output:
        raise AssertionError(f"expected forbidden naming finding, got code={code}, stderr={output!r}")


def main() -> int:
    tests = [
        test_load_audit_parses_repo_local_yaml_subset,
        test_load_audit_handles_inline_comments_and_single_quotes,
        test_load_audit_rejects_unsupported_yaml_subset,
        test_word_regex_is_bounded_to_identifier_words,
        test_scan_paths_excludes_audit_target_git_and_reviews,
        test_main_reports_forbidden_and_required_names,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 naming verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
