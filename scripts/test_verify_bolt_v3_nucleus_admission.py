#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 nucleus admission audit."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_nucleus_admission.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bolt_v3_nucleus_admission", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_fixture(root: Path, files: dict[str, str]) -> None:
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


def provider_leaks_fixture(allowances: str = "FINDING_ALLOWANCES = ()\n") -> str:
    return (
        allowances
        + "\n"
        + "def production_text(text: str) -> str:\n"
        + "    output = []\n"
        + "    skip_cfg_test = False\n"
        + "    saw_brace = False\n"
        + "    depth = 0\n"
        + "    for line in text.splitlines():\n"
        + "        stripped = line.strip()\n"
        + "        if stripped.startswith('//'):\n"
        + "            output.append('')\n"
        + "            continue\n"
        + "        if stripped.startswith('#[cfg(test)]'):\n"
        + "            skip_cfg_test = True\n"
        + "            saw_brace = False\n"
        + "            depth = 0\n"
        + "            output.append('')\n"
        + "            continue\n"
        + "        if skip_cfg_test:\n"
        + "            output.append('')\n"
        + "            depth += line.count('{') - line.count('}')\n"
        + "            saw_brace = saw_brace or '{' in line\n"
        + "            if saw_brace and depth <= 0:\n"
        + "                skip_cfg_test = False\n"
        + "            continue\n"
        + "        output.append(line)\n"
        + "    return '\\n'.join(output)\n"
    )


def admitted_files() -> dict[str, str]:
    return {
        "src/bolt_v3_contracts.rs": """
            pub struct ProviderContract;
            pub struct MarketFamilyContract;
            pub struct StrategyArchetypeContract;
            pub struct DecisionEvent;
            pub trait CustomDataTrait {}
            pub fn ensure_custom_data_registered() {}
            pub struct ConformanceHarness;
            pub struct BacktestEngineLiveParityBoundary;
            pub fn add_strategy() {}
        """,
        "src/bolt_v3_adapters.rs": """
            pub struct GenericAdapterMapping;
        """,
        "src/bolt_v3_providers/mod.rs": """
            pub struct ProviderBinding;
        """,
        "src/bolt_v3_market_families/mod.rs": """
            pub struct MarketFamilyBinding;
        """,
        "src/bolt_v3_archetypes/mod.rs": """
            pub struct StrategyArchetypeBinding;
        """,
        "scripts/verify_bolt_v3_provider_leaks.py": provider_leaks_fixture(),
        "justfile": """
            verify-bolt-v3-nucleus-admission:
                python3 scripts/verify_bolt_v3_nucleus_admission.py
        """,
        ".github/workflows/ci.yml": """
            name: ci
        """,
    }


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def blocker_classes(run) -> set[str]:
    return {blocker.blocker_class for blocker in run.blockers}


def verdict_from_output(output: str) -> str:
    for line in output.splitlines():
        if line.startswith("VERDICT: "):
            return line.removeprefix("VERDICT: ")
    raise AssertionError(f"missing verdict in output:\n{output}")


def reset_provider_leaks_cache(verifier) -> None:
    for module_name in list(sys.modules):
        if module_name.startswith("_bolt_v3_provider_leaks_"):
            sys.modules.pop(module_name, None)
    verifier._PROVIDER_LEAKS_MODULES.clear()
    verifier._PROVIDER_LEAKS_LOAD_ERRORS.clear()


def test_current_repo_report_mode_reports_known_blockers_and_exits_zero() -> None:
    result = run_script()

    assert result.returncode == 0
    assert verdict_from_output(result.stdout) in {"ADMITTED", "BLOCKED", "UNSCANNABLE"}
    assert "scan-universe:" in result.stdout


def test_current_repo_strict_mode_reports_known_blockers_and_exits_nonzero() -> None:
    report = run_script()
    result = run_script("--strict")
    expected_returncode = 0 if verdict_from_output(report.stdout) == "ADMITTED" else 1

    assert result.returncode == expected_returncode
    assert verdict_from_output(result.stdout) == verdict_from_output(report.stdout)


def test_admitted_fixture_has_no_blockers_and_strict_mode_exits_zero() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(root, admitted_files())

        verifier = load_verifier()
        run = verifier.audit_root(root)
        assert run.verdict == "ADMITTED"
        assert run.blockers == ()

        result = run_script("--repo-root", str(root), "--strict")
        assert result.returncode == 0
        assert "VERDICT: ADMITTED" in result.stdout


def test_scan_universe_failure_blocks_admission() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert run.verdict == "UNSCANNABLE"
        assert "scan-universe-unproven" in blocker_classes(run)


def test_scan_universe_includes_required_path_groups() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "tests/bolt_v3_contract.rs": "fn test_contract() {}\n",
                "tests/bolt_v3/nested_contract.rs": "fn nested_contract() {}\n",
                "tests/fixtures/bolt_v3/root.toml": "kind = \"polymarket\"\n",
                "docs/bolt-v3/evidence.md": "Bolt-v3 evidence note\n",
            },
        )

        verifier = load_verifier()
        universe = verifier.discover_scan_universe(root)
        paths = {path.relative_to(root).as_posix() for path in universe.files}

        assert "src/bolt_v3_contracts.rs" in paths
        assert "tests/bolt_v3_contract.rs" in paths
        assert "tests/bolt_v3/nested_contract.rs" in paths
        assert "tests/fixtures/bolt_v3/root.toml" in paths
        assert "docs/bolt-v3/evidence.md" in paths
        assert "scripts/verify_bolt_v3_provider_leaks.py" in paths
        assert "justfile" in paths
        assert ".github/workflows/ci.yml" in paths


def test_generic_updown_plan_and_clock_in_core_are_blockers() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_adapters.rs": """
                    use crate::bolt_v3_market_families::updown::MarketIdentityPlan;
                    pub type BoltV3UpdownNowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "generic-contract-leak" in blocker_classes(run)
        rendered = "\n".join(blocker.render() for blocker in run.blockers)
        assert "MarketIdentityPlan" in rendered
        assert "BoltV3UpdownNowFn" in rendered


def test_missing_decision_conformance_and_backtest_surfaces_are_blockers() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_contracts.rs": """
                    pub struct ProviderContract;
                    pub struct MarketFamilyContract;
                    pub struct StrategyArchetypeContract;
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "missing-contract-surface" in blocker_classes(run)
        rendered = "\n".join(blocker.render() for blocker in run.blockers)
        assert "DecisionEvent" in rendered
        assert "BacktestEngine" in rendered
        assert "conformance" in rendered


def test_missing_contract_surfaces_ignore_comments_and_cfg_test_only_mentions() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_contracts.rs": """
                    // DecisionEvent, CustomDataTrait, ensure_custom_data_registered.
                    // ConformanceHarness and BacktestEngineLiveParityBoundary.
                    #[cfg(test)]
                    mod tests {
                        struct DecisionEvent;
                        struct ConformanceHarness;
                        struct BacktestEngineLiveParityBoundary;
                    }
                    pub struct ProviderContract;
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "missing-contract-surface" in blocker_classes(run)
        rendered = "\n".join(blocker.render() for blocker in run.blockers)
        assert "DecisionEvent" in rendered
        assert "BacktestEngine" in rendered
        assert "conformance" in rendered


def test_missing_contract_surfaces_do_not_accept_identifier_substrings() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_contracts.rs": """
                    pub struct NotDecisionEventYet;
                    pub struct ConformanceHarnessed;
                    pub struct BacktestEngineLiveParityBoundaryValue;
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "missing-contract-surface" in blocker_classes(run)


def test_missing_contract_surfaces_require_each_decision_event_term() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_contracts.rs": """
                    pub struct ProviderContract;
                    pub struct DecisionEvent;
                    pub struct ConformanceHarness;
                    pub struct BacktestEngineLiveParityBoundary;
                    pub fn add_strategy() {}
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "missing-contract-surface" in blocker_classes(run)


def test_missing_contract_surfaces_ignore_test_only_contract_names() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_contracts.rs": """
                    pub struct ProviderContract;
                """,
                "tests/bolt_v3_contracts.rs": """
                    pub struct DecisionEvent;
                    pub trait CustomDataTrait {}
                    pub fn ensure_custom_data_registered() {}
                    pub struct ConformanceHarness;
                    pub struct BacktestEngineLiveParityBoundary;
                    pub fn add_strategy() {}
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "missing-contract-surface" in blocker_classes(run)


def test_unowned_default_and_provider_leak_allowlist_are_blockers() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_client_registration.rs": """
                    pub fn config() {
                        transport_backend: Default::default(),
                    }
                """,
                "scripts/verify_bolt_v3_provider_leaks.py": provider_leaks_fixture(
                    'FINDING_ALLOWANCES = (\n'
                    '    "bolt_v3_market_families::updown::MarketIdentityPlan,",\n'
                    '    "pub type BoltV3UpdownNowFn = Arc<dyn Fn() -> i64 + Send + Sync>;",\n'
                    ")\n"
                ),
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "unowned-runtime-default" in blocker_classes(run)
        assert "narrow-verifier-bypass" in blocker_classes(run)


def test_unowned_default_inside_contains_or_concat_is_still_a_blocker() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_client_registration.rs": """
                    pub fn config() {
                        assert!(!source.contains(concat!("..", "Default::default()")));
                        transport_backend: Default::default(),
                    }
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        default_records = [
            record
            for blocker in run.blockers
            if blocker.blocker_id == "unowned-runtime-default"
            for record in blocker.evidence
            if record.excerpt and "Default::default" in record.excerpt
        ]
        assert len(default_records) == 2


def test_narrow_verifier_bypass_scans_only_finding_allowances() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "scripts/verify_bolt_v3_provider_leaks.py": provider_leaks_fixture(
                    "FINDING_ALLOWANCES = ()\n"
                    "# MarketIdentityPlan and BoltV3UpdownNowFn are discussed here,\n"
                    "# but they are not active allowances.\n"
                ),
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "narrow-verifier-bypass" not in blocker_classes(run)


def test_narrow_verifier_bypass_scans_annotated_finding_allowances() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "scripts/verify_bolt_v3_provider_leaks.py": provider_leaks_fixture(
                    'FINDING_ALLOWANCES: tuple[str, ...] = (\n'
                    '    "pub type BoltV3UpdownNowFn = Arc<dyn Fn() -> i64 + Send + Sync>;",\n'
                    ")\n"
                ),
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "narrow-verifier-bypass" in blocker_classes(run)


def test_fenced_fixtures_and_provider_owned_bindings_are_allowed_contexts() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "tests/fixtures/bolt_v3/root.toml": """
                    [venues.polymarket_main]
                    kind = "polymarket"
                """,
                "tests/fixtures/bolt_v3/strategies/binary_oracle.toml": """
                    strategy_archetype = "binary_oracle_edge_taker"
                    rotating_market_family = "updown"
                    underlying_asset = "BTC"
                """,
                "src/bolt_v3_providers/polymarket.rs": """
                    pub const KEY: &str = "polymarket";
                """,
                "src/bolt_v3_market_families/updown.rs": """
                    pub const KEY: &str = "updown";
                    pub struct MarketIdentityPlan;
                """,
                "src/bolt_v3_archetypes/binary_oracle_edge_taker.rs": """
                    pub const KEY: &str = "binary_oracle_edge_taker";
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "unfenced-concrete-fixture" not in blocker_classes(run)
        assert "generic-contract-leak" not in blocker_classes(run)


def test_unfenced_fixture_values_are_blockers() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "tests/fixtures/random/root.toml": """
                    [venues.polymarket_main]
                    kind = "polymarket"
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "unfenced-concrete-fixture" in blocker_classes(run)


def test_unfenced_fixture_values_are_case_insensitive() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "tests/fixtures/random/root.toml": """
                    family = "UpDown"
                    symbol = "btcusdt"
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "unfenced-concrete-fixture" in blocker_classes(run)


def test_unfenced_fixture_values_do_not_match_concrete_token_substrings() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "tests/fixtures/random/root.toml": """
                    venue = "polymarketing"
                    feed = "ChainlinkProtocol"
                    symbol = "BTCMARKETS"
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "unfenced-concrete-fixture" not in blocker_classes(run)


def test_invalid_waivers_are_blockers() -> None:
    verifier = load_verifier()
    invalid = verifier.Waiver(
        blocker_id="generic-contract-leak",
        path="",
        excerpt="MarketIdentityPlan",
        rationale="temporary",
        retirement_issue="#290",
    )

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(root, admitted_files())
        run = verifier.audit_root(root, waivers=[invalid])

        assert "invalid-waiver" in blocker_classes(run)


def test_waiver_removes_only_matching_evidence_record() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_adapters.rs": """
                    use crate::bolt_v3_market_families::updown::MarketIdentityPlan;
                    pub type BoltV3UpdownNowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
                """,
            },
        )

        verifier = load_verifier()
        first_run = verifier.audit_root(root)
        blocker = next(blocker for blocker in first_run.blockers if blocker.blocker_id == "generic-contract-leak")
        target = next(record for record in blocker.evidence if record.excerpt and "MarketIdentityPlan" in record.excerpt)
        waiver = verifier.Waiver(
            blocker_id="generic-contract-leak",
            path=target.path,
            excerpt=target.excerpt,
            rationale="temporary fixture exception",
            retirement_issue="#290",
        )

        waived_run = verifier.audit_root(root, waivers=[waiver])

        generic_blocker = next(blocker for blocker in waived_run.blockers if blocker.blocker_id == "generic-contract-leak")
        rendered = generic_blocker.render()
        assert "MarketIdentityPlan" not in rendered
        assert "BoltV3UpdownNowFn" in rendered


def test_provider_leaks_import_failure_blocks_and_strips_comments() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "src/bolt_v3_contracts.rs": """
                    /*
                     DecisionEvent, CustomDataTrait, ensure_custom_data_registered.
                     ConformanceHarness and BacktestEngineLiveParityBoundary.
                    */
                    pub struct ProviderContract;
                """,
                "scripts/verify_bolt_v3_provider_leaks.py": 'raise RuntimeError("boom")\n',
            },
        )

        verifier = load_verifier()
        reset_provider_leaks_cache(verifier)
        run = verifier.audit_root(root)
        reset_provider_leaks_cache(verifier)

        assert "scan-universe-unproven" in blocker_classes(run)
        assert "missing-contract-surface" in blocker_classes(run)
        assert "RuntimeError: boom" in run.render()
        assert not any(module_name.startswith("_bolt_v3_provider_leaks_") for module_name in sys.modules)


def test_provider_leaks_dependency_is_probed_even_without_rust_scan() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(root, {"scripts/verify_bolt_v3_provider_leaks.py": 'raise RuntimeError("boom")\n'})

        verifier = load_verifier()
        reset_provider_leaks_cache(verifier)
        run = verifier.audit_root(root)
        reset_provider_leaks_cache(verifier)

        rendered = run.render()
        assert "provider-leaks-production-text-unavailable" in rendered
        assert "RuntimeError: boom" in rendered


def test_degraded_rust_scan_is_blank_when_provider_helper_fails() -> None:
    verifier = load_verifier()
    text = 'pub const RAW: &str = r#"embedded " // BoltV3UpdownNowFn"#;\n// MarketIdentityPlan\n'

    stripped = verifier.blank_preserving_lines(text)

    assert "BoltV3UpdownNowFn" not in stripped
    assert "MarketIdentityPlan" not in stripped
    assert stripped.count("\n") == text.count("\n")


def test_fixture_path_detection_uses_directory_segments() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            admitted_files()
            | {
                "tests/notfixtures/random.toml": """
                    kind = "polymarket"
                """,
            },
        )

        verifier = load_verifier()
        run = verifier.audit_root(root)

        assert "unfenced-concrete-fixture" not in blocker_classes(run)


def test_secret_like_excerpts_are_redacted() -> None:
    verifier = load_verifier()
    excerpts = [
        verifier.safe_excerpt('api_key = "abc123secret"'),
        verifier.safe_excerpt("api_secret = 'singlequotedsecret'"),
        verifier.safe_excerpt("token = bare-secret-token"),
        verifier.safe_excerpt('"private_key": "jsonsecret"'),
    ]

    rendered = "\n".join(excerpts)
    assert "abc123secret" not in rendered
    assert "singlequotedsecret" not in rendered
    assert "bare-secret-token" not in rendered
    assert "jsonsecret" not in rendered
    assert rendered.count("<redacted>") == 4


def main() -> int:
    tests = [
        test_current_repo_report_mode_reports_known_blockers_and_exits_zero,
        test_current_repo_strict_mode_reports_known_blockers_and_exits_nonzero,
        test_admitted_fixture_has_no_blockers_and_strict_mode_exits_zero,
        test_scan_universe_failure_blocks_admission,
        test_scan_universe_includes_required_path_groups,
        test_generic_updown_plan_and_clock_in_core_are_blockers,
        test_missing_decision_conformance_and_backtest_surfaces_are_blockers,
        test_missing_contract_surfaces_ignore_comments_and_cfg_test_only_mentions,
        test_missing_contract_surfaces_do_not_accept_identifier_substrings,
        test_missing_contract_surfaces_require_each_decision_event_term,
        test_missing_contract_surfaces_ignore_test_only_contract_names,
        test_unowned_default_and_provider_leak_allowlist_are_blockers,
        test_unowned_default_inside_contains_or_concat_is_still_a_blocker,
        test_narrow_verifier_bypass_scans_only_finding_allowances,
        test_narrow_verifier_bypass_scans_annotated_finding_allowances,
        test_fenced_fixtures_and_provider_owned_bindings_are_allowed_contexts,
        test_unfenced_fixture_values_are_blockers,
        test_unfenced_fixture_values_are_case_insensitive,
        test_unfenced_fixture_values_do_not_match_concrete_token_substrings,
        test_invalid_waivers_are_blockers,
        test_waiver_removes_only_matching_evidence_record,
        test_provider_leaks_import_failure_blocks_and_strips_comments,
        test_provider_leaks_dependency_is_probed_even_without_rust_scan,
        test_degraded_rust_scan_is_blank_when_provider_helper_fails,
        test_fixture_path_detection_uses_directory_segments,
        test_secret_like_excerpts_are_redacted,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 nucleus admission audit self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
