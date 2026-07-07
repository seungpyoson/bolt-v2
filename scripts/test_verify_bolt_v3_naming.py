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


def test_load_audit_uses_pyyaml_for_standard_yaml_features() -> None:
    original_audit_path = VERIFIER.AUDIT_PATH
    audit_text = """
audit_id: &audit_id probe
version: 1
notes: |
  multiline note
rules:
  - name: "fixture"
    include_globs: &rust_globs
      - "src/**/*.rs"
renamed_in_current_audit:
  - from: "VenueKind"
    to: "ProviderKey"
defensive_forbidden: []
path_scoped_forbidden:
  - from: "MarketSlugFilter"
    to: "ProviderOwnedFilter"
    include_globs: *rust_globs
    reason: >
      provider
      boundary
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

    scoped = audit["path_scoped_forbidden"][0]
    if scoped["include_globs"] != ["src/**/*.rs"] or scoped["reason"].strip() != "provider boundary":
        raise AssertionError(f"standard YAML parse failed: {scoped!r}")
    if audit["notes"] != "multiline note\n":
        raise AssertionError(f"block scalar parse failed: {audit!r}")


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


def test_word_regex_is_bounded_to_identifier_words() -> None:
    regex = VERIFIER.word_re("VenueKind")
    if not regex.search("VenueKind::Polymarket"):
        raise AssertionError("expected exact identifier match")
    if regex.search("LegacyVenueKindName"):
        raise AssertionError("unexpected subword match")


def test_word_regex_matches_terms_with_trailing_punctuation() -> None:
    regex = VERIFIER.word_re("[venues.")
    if not regex.search("[venues.polymarket_main]"):
        raise AssertionError("expected dotted table-prefix match")


def test_matches_any_treats_globstar_as_zero_or_more_directories() -> None:
    original_root = VERIFIER.REPO_ROOT
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        direct = root / "config" / "root.toml"
        nested = root / "config" / "live" / "root.toml"
        deeply_nested = root / "config" / "live" / "prod" / "root.toml"
        direct.parent.mkdir(parents=True)
        nested.parent.mkdir(parents=True)
        deeply_nested.parent.mkdir(parents=True)
        direct.write_text("probe\n", encoding="utf-8")
        nested.write_text("probe\n", encoding="utf-8")
        deeply_nested.write_text("probe\n", encoding="utf-8")
        try:
            VERIFIER.REPO_ROOT = root
            pattern = ["config/**/*.toml"]
            if not VERIFIER.matches_any(direct, pattern):
                raise AssertionError("globstar should match zero nested directories")
            if not VERIFIER.matches_any(nested, pattern):
                raise AssertionError("globstar should match nested directories")
            if not VERIFIER.matches_any(deeply_nested, pattern):
                raise AssertionError("globstar should match deeper nested directories")
        finally:
            VERIFIER.REPO_ROOT = original_root


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


def test_default_scan_paths_cover_companion_docs_and_research_artifacts() -> None:
    scanned = {path.relative_to(VERIFIER.REPO_ROOT).as_posix() for path in VERIFIER.scan_paths()}
    required = {
        "docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md",
        "docs/bolt-v3/2026-04-28-source-grounded-status-map.md",
        "docs/bolt-v3/2026-05-18-production-readiness-contract.md",
        "docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml",
    }
    missing = required - scanned
    if missing:
        raise AssertionError(f"default naming scan missing {sorted(missing)}")


def test_main_reports_forbidden_names() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    original_scan_globs = VERIFIER.SCAN_GLOBS
    original_excluded = VERIFIER.EXCLUDED_RELATIVE_PATHS
    original_allowlist_path = VERIFIER.MISNOMER_ALLOWLIST_PATH
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "audit.yaml"
        audit_path.write_text(AUDIT_TEXT, encoding="utf-8")
        source = root / "src" / "core.rs"
        source.parent.mkdir(parents=True)
        source.write_text("pub type X = VenueKind;\n", encoding="utf-8")
        allowlist_path = root / "allowlist.txt"
        allowlist_path.write_text("# no allowed residuals\n", encoding="utf-8")
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            VERIFIER.SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.EXCLUDED_RELATIVE_PATHS = set()
            VERIFIER.MISNOMER_ALLOWLIST_PATH = allowlist_path
            with contextlib.redirect_stderr(stderr):
                code = VERIFIER.main()
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path
            VERIFIER.SCAN_GLOBS = original_scan_globs
            VERIFIER.EXCLUDED_RELATIVE_PATHS = original_excluded
            VERIFIER.MISNOMER_ALLOWLIST_PATH = original_allowlist_path

    output = stderr.getvalue()
    if code != 1 or "forbidden 'VenueKind'" not in output:
        raise AssertionError(f"expected forbidden naming finding, got code={code}, stderr={output!r}")


def test_main_reports_missing_audit_without_traceback() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    stderr = io.StringIO()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "missing-audit.yaml"
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            with contextlib.redirect_stderr(stderr):
                try:
                    code = VERIFIER.main()
                except FileNotFoundError as exc:
                    raise AssertionError("missing naming audit should be reported normally") from exc
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path

    output = stderr.getvalue()
    expected = f"FAIL: missing Bolt-v3 naming audit file: {audit_path}\n"
    if code != 1 or output != expected:
        raise AssertionError(f"expected missing audit finding, got code={code}, stderr={output!r}")


def test_main_reports_non_mapping_audit_without_traceback() -> None:
    for audit_text, type_name in [
        ("- item\n", "list"),
        ("justastring\n", "str"),
    ]:
        original_root = VERIFIER.REPO_ROOT
        original_audit_path = VERIFIER.AUDIT_PATH
        stderr = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            audit_path = root / "audit.yaml"
            audit_path.write_text(audit_text, encoding="utf-8")
            try:
                VERIFIER.REPO_ROOT = root
                VERIFIER.AUDIT_PATH = audit_path
                with contextlib.redirect_stderr(stderr):
                    try:
                        code = VERIFIER.main()
                    except Exception as exc:
                        raise AssertionError(
                            "shape-invalid naming audit should be reported normally"
                        ) from exc
            finally:
                VERIFIER.REPO_ROOT = original_root
                VERIFIER.AUDIT_PATH = original_audit_path

        output = stderr.getvalue()
        expected = (
            "FAIL: invalid Bolt-v3 naming audit file: "
            f"expected a mapping, got {type_name}\n"
        )
        if code != 1 or output != expected:
            raise AssertionError(
                f"expected invalid audit finding, got code={code}, stderr={output!r}"
            )


def test_main_reports_unreadable_audit_without_traceback() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    stderr = io.StringIO()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "audit.yaml"
        audit_path.mkdir()
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            with contextlib.redirect_stderr(stderr):
                try:
                    code = VERIFIER.main()
                except OSError as exc:
                    raise AssertionError("unreadable naming audit should be reported normally") from exc
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path

    output = stderr.getvalue()
    expected = f"FAIL: unreadable Bolt-v3 naming audit file: {audit_path}\n"
    if code != 1 or output != expected:
        raise AssertionError(f"expected unreadable audit finding, got code={code}, stderr={output!r}")


def test_main_reports_undecodable_audit_without_traceback() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    stderr = io.StringIO()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "audit.yaml"
        audit_path.write_bytes(b"\xff\xfe\x00")
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            with contextlib.redirect_stderr(stderr):
                try:
                    code = VERIFIER.main()
                except UnicodeDecodeError as exc:
                    raise AssertionError("undecodable naming audit should be reported normally") from exc
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path

    output = stderr.getvalue()
    expected = f"FAIL: invalid Bolt-v3 naming audit file: {audit_path} is not valid UTF-8\n"
    if code != 1 or output != expected:
        raise AssertionError(f"expected undecodable audit finding, got code={code}, stderr={output!r}")


def test_main_reports_path_scoped_forbidden_table_prefix() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    original_scan_globs = VERIFIER.SCAN_GLOBS
    original_excluded = VERIFIER.EXCLUDED_RELATIVE_PATHS
    original_allowlist_path = VERIFIER.MISNOMER_ALLOWLIST_PATH
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "audit.yaml"
        audit_path.write_text(
            """
audit_id: "probe"
version: 1
renamed_in_current_audit: []
defensive_forbidden: []
path_scoped_forbidden:
  - from: "[venues."
    to: "[clients."
    include_globs:
      - "tests/fixtures/**/*.toml"
    reason: "client table"
accepted_non_nt_names: []
""".lstrip(),
            encoding="utf-8",
        )
        source = root / "tests" / "fixtures" / "root.toml"
        source.parent.mkdir(parents=True)
        source.write_text("[venues.polymarket_main]\n", encoding="utf-8")
        allowlist_path = root / "allowlist.txt"
        allowlist_path.write_text("# no allowed residuals\n", encoding="utf-8")
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            VERIFIER.SCAN_GLOBS = ["tests/fixtures/**/*.toml"]
            VERIFIER.EXCLUDED_RELATIVE_PATHS = set()
            VERIFIER.MISNOMER_ALLOWLIST_PATH = allowlist_path
            with contextlib.redirect_stderr(stderr):
                code = VERIFIER.main()
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path
            VERIFIER.SCAN_GLOBS = original_scan_globs
            VERIFIER.EXCLUDED_RELATIVE_PATHS = original_excluded
            VERIFIER.MISNOMER_ALLOWLIST_PATH = original_allowlist_path

    output = stderr.getvalue()
    if code != 1 or "forbidden '[venues.'" not in output:
        raise AssertionError(f"expected path-scoped table finding, got code={code}, stderr={output!r}")


def test_main_fails_closed_when_scan_paths_are_empty() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    original_scan_globs = VERIFIER.SCAN_GLOBS
    original_misnomer_scan_globs = VERIFIER.MISNOMER_SCAN_GLOBS
    original_excluded = VERIFIER.EXCLUDED_RELATIVE_PATHS
    original_allowlist_path = VERIFIER.MISNOMER_ALLOWLIST_PATH
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "audit.yaml"
        audit_path.write_text(AUDIT_TEXT, encoding="utf-8")
        allowlist_path = root / "allowlist.txt"
        allowlist_path.write_text("# no allowed residuals\n", encoding="utf-8")
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            VERIFIER.SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.MISNOMER_SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.EXCLUDED_RELATIVE_PATHS = set()
            VERIFIER.MISNOMER_ALLOWLIST_PATH = allowlist_path
            with contextlib.redirect_stderr(stderr):
                code = VERIFIER.main()
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path
            VERIFIER.SCAN_GLOBS = original_scan_globs
            VERIFIER.MISNOMER_SCAN_GLOBS = original_misnomer_scan_globs
            VERIFIER.EXCLUDED_RELATIVE_PATHS = original_excluded
            VERIFIER.MISNOMER_ALLOWLIST_PATH = original_allowlist_path

    output = stderr.getvalue()
    expected = (
        "FAIL: Bolt-v3 naming scan paths: enforcement set is empty\n"
        "FAIL: capital-admission misnomer scan paths: enforcement set is empty\n"
    )
    if code != 1 or output != expected:
        raise AssertionError(f"expected empty scan floor finding, got code={code}, stderr={output!r}")


def test_main_fails_closed_when_misnomer_scan_paths_are_empty_before_naming_scan() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    original_scan_globs = VERIFIER.SCAN_GLOBS
    original_misnomer_scan_globs = VERIFIER.MISNOMER_SCAN_GLOBS
    original_excluded = VERIFIER.EXCLUDED_RELATIVE_PATHS
    original_allowlist_path = VERIFIER.MISNOMER_ALLOWLIST_PATH
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "audit.yaml"
        audit_path.write_text(AUDIT_TEXT, encoding="utf-8")
        source = root / "src" / "bad.rs"
        source.parent.mkdir(parents=True)
        source.write_text("pub struct VenueKind;\n", encoding="utf-8")
        allowlist_path = root / "allowlist.txt"
        allowlist_path.write_text("# no allowed residuals\n", encoding="utf-8")
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            VERIFIER.SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.MISNOMER_SCAN_GLOBS = ["docs/**/*.md"]
            VERIFIER.EXCLUDED_RELATIVE_PATHS = set()
            VERIFIER.MISNOMER_ALLOWLIST_PATH = allowlist_path
            with contextlib.redirect_stderr(stderr):
                code = VERIFIER.main()
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path
            VERIFIER.SCAN_GLOBS = original_scan_globs
            VERIFIER.MISNOMER_SCAN_GLOBS = original_misnomer_scan_globs
            VERIFIER.EXCLUDED_RELATIVE_PATHS = original_excluded
            VERIFIER.MISNOMER_ALLOWLIST_PATH = original_allowlist_path

    output = stderr.getvalue()
    expected = "FAIL: capital-admission misnomer scan paths: enforcement set is empty\n"
    if code != 1 or output != expected:
        raise AssertionError(
            f"expected terminal misnomer scan floor, got code={code}, stderr={output!r}"
        )


def test_main_fails_closed_when_audit_rule_rows_are_empty() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    original_scan_globs = VERIFIER.SCAN_GLOBS
    original_misnomer_scan_globs = VERIFIER.MISNOMER_SCAN_GLOBS
    original_excluded = VERIFIER.EXCLUDED_RELATIVE_PATHS
    original_allowlist_path = VERIFIER.MISNOMER_ALLOWLIST_PATH
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "audit.yaml"
        audit_path.write_text(
            """
audit_id: "probe"
version: 1
renamed_in_current_audit: []
defensive_forbidden: []
path_scoped_forbidden: []
accepted_non_nt_names: []
""".lstrip(),
            encoding="utf-8",
        )
        source = root / "src" / "clean.rs"
        source.parent.mkdir(parents=True)
        source.write_text("pub struct Clean;\n", encoding="utf-8")
        allowlist_path = root / "allowlist.txt"
        allowlist_path.write_text(
            "src/clean.rs:1\tpub struct PositionSizer;\tfixture stale entry\n",
            encoding="utf-8",
        )
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            VERIFIER.SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.MISNOMER_SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.EXCLUDED_RELATIVE_PATHS = set()
            VERIFIER.MISNOMER_ALLOWLIST_PATH = allowlist_path
            with contextlib.redirect_stderr(stderr):
                code = VERIFIER.main()
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path
            VERIFIER.SCAN_GLOBS = original_scan_globs
            VERIFIER.MISNOMER_SCAN_GLOBS = original_misnomer_scan_globs
            VERIFIER.EXCLUDED_RELATIVE_PATHS = original_excluded
            VERIFIER.MISNOMER_ALLOWLIST_PATH = original_allowlist_path

    output = stderr.getvalue()
    expected = "FAIL: Bolt-v3 naming audit rule rows: enforcement set is empty\n"
    if code != 1 or output != expected:
        raise AssertionError(f"expected empty rule-row floor, got code={code}, stderr={output!r}")


def run_main_with_misnomer_fixture(
    files: dict[str, str],
    allowlist_text: str | None,
) -> tuple[int, str]:
    original_root = VERIFIER.REPO_ROOT
    original_audit_path = VERIFIER.AUDIT_PATH
    original_scan_globs = VERIFIER.SCAN_GLOBS
    original_misnomer_scan_globs = VERIFIER.MISNOMER_SCAN_GLOBS
    original_excluded = VERIFIER.EXCLUDED_RELATIVE_PATHS
    original_allowlist_path = VERIFIER.MISNOMER_ALLOWLIST_PATH
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        audit_path = root / "audit.yaml"
        audit_path.write_text(AUDIT_TEXT, encoding="utf-8")
        allowlist_path = root / "allowlist.txt"
        if allowlist_text is not None:
            allowlist_path.write_text(allowlist_text, encoding="utf-8")
        for rel, content in files.items():
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        stderr = io.StringIO()
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.AUDIT_PATH = audit_path
            VERIFIER.SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.MISNOMER_SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.EXCLUDED_RELATIVE_PATHS = set()
            VERIFIER.MISNOMER_ALLOWLIST_PATH = allowlist_path
            with contextlib.redirect_stderr(stderr):
                code = VERIFIER.main()
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.AUDIT_PATH = original_audit_path
            VERIFIER.SCAN_GLOBS = original_scan_globs
            VERIFIER.MISNOMER_SCAN_GLOBS = original_misnomer_scan_globs
            VERIFIER.EXCLUDED_RELATIVE_PATHS = original_excluded
            VERIFIER.MISNOMER_ALLOWLIST_PATH = original_allowlist_path
    return code, stderr.getvalue()


def test_capital_admission_misnomer_fence_catches_screaming_snake() -> None:
    code, output = run_main_with_misnomer_fixture(
        {
            "src/core.rs": (
                'const BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID: &str = "probe";\n'
            )
        },
        "# no allowed residuals\n",
    )

    if code != 1 or "POSITION_SIZER" not in output or "capital-admission misnomer" not in output:
        raise AssertionError(f"expected SCREAMING_SNAKE misnomer finding, got {code}, {output!r}")


def test_capital_admission_misnomer_fence_allows_legitimate_sizer_keep_list() -> None:
    code, output = run_main_with_misnomer_fixture(
        {
            "src/bolt_v3_sizing.rs": (
                "pub struct SizingPolicyProbe;\n"
                "pub fn choose_robust_size() -> RobustSizeProbe { todo!() }\n"
            )
        },
        "# no allowed residuals\n",
    )

    if code != 0:
        raise AssertionError(f"expected legitimate sizer keep-list to pass, got {code}, {output!r}")


def test_capital_admission_misnomer_scan_paths_floor_is_exact() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_misnomer_scan_globs = VERIFIER.MISNOMER_SCAN_GLOBS
    original_allowlist_path = VERIFIER.MISNOMER_ALLOWLIST_PATH
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        allowlist_path = root / "allowlist.txt"
        allowlist_path.write_text("# no allowed residuals\n", encoding="utf-8")
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.MISNOMER_SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.MISNOMER_ALLOWLIST_PATH = allowlist_path
            findings = VERIFIER.verify_capital_admission_misnomers()
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.MISNOMER_SCAN_GLOBS = original_misnomer_scan_globs
            VERIFIER.MISNOMER_ALLOWLIST_PATH = original_allowlist_path

    expected = ["capital-admission misnomer scan paths: enforcement set is empty"]
    if findings != expected:
        raise AssertionError(f"expected exact misnomer scan floor, got {findings!r}")


def test_capital_admission_misnomer_scan_paths_floor_precedes_missing_allowlist() -> None:
    original_root = VERIFIER.REPO_ROOT
    original_misnomer_scan_globs = VERIFIER.MISNOMER_SCAN_GLOBS
    original_allowlist_path = VERIFIER.MISNOMER_ALLOWLIST_PATH
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        try:
            VERIFIER.REPO_ROOT = root
            VERIFIER.MISNOMER_SCAN_GLOBS = ["src/**/*.rs"]
            VERIFIER.MISNOMER_ALLOWLIST_PATH = root / "missing-allowlist.txt"
            findings = VERIFIER.verify_capital_admission_misnomers()
        finally:
            VERIFIER.REPO_ROOT = original_root
            VERIFIER.MISNOMER_SCAN_GLOBS = original_misnomer_scan_globs
            VERIFIER.MISNOMER_ALLOWLIST_PATH = original_allowlist_path

    expected = ["capital-admission misnomer scan paths: enforcement set is empty"]
    if findings != expected:
        raise AssertionError(f"expected scan floor before missing allowlist, got {findings!r}")


def test_capital_admission_misnomer_fence_fails_closed_without_allowlist() -> None:
    code, output = run_main_with_misnomer_fixture(
        {"src/core.rs": "pub struct CapitalAdmissionOnly;\n"},
        None,
    )

    if code != 1 or "missing capital-admission misnomer allowlist" not in output:
        raise AssertionError(f"expected missing allowlist failure, got {code}, {output!r}")


def main() -> int:
    tests = [
        test_load_audit_uses_pyyaml_for_standard_yaml_features,
        test_load_audit_handles_inline_comments_and_single_quotes,
        test_word_regex_is_bounded_to_identifier_words,
        test_word_regex_matches_terms_with_trailing_punctuation,
        test_matches_any_treats_globstar_as_zero_or_more_directories,
        test_scan_paths_excludes_audit_target_git_and_reviews,
        test_default_scan_paths_cover_companion_docs_and_research_artifacts,
        test_main_reports_forbidden_names,
        test_main_reports_missing_audit_without_traceback,
        test_main_reports_non_mapping_audit_without_traceback,
        test_main_reports_unreadable_audit_without_traceback,
        test_main_reports_undecodable_audit_without_traceback,
        test_main_reports_path_scoped_forbidden_table_prefix,
        test_main_fails_closed_when_scan_paths_are_empty,
        test_main_fails_closed_when_misnomer_scan_paths_are_empty_before_naming_scan,
        test_main_fails_closed_when_audit_rule_rows_are_empty,
        test_capital_admission_misnomer_fence_catches_screaming_snake,
        test_capital_admission_misnomer_fence_allows_legitimate_sizer_keep_list,
        test_capital_admission_misnomer_scan_paths_floor_is_exact,
        test_capital_admission_misnomer_scan_paths_floor_precedes_missing_allowlist,
        test_capital_admission_misnomer_fence_fails_closed_without_allowlist,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 naming verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
