#!/usr/bin/env python3
"""Self-tests for verify_bolt_v3_boundary_evidence.py."""

from __future__ import annotations

import datetime as dt
import importlib.util
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

from ci_workflow_hygiene_test_helpers import init_fixture_repo, repo_git_command


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_boundary_evidence.py"


def fixture_nt_revision() -> str:
    manifest = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    dependencies = manifest.get("dependencies", {})
    revisions = {
        specification.get("rev")
        for specification in dependencies.values()
        if isinstance(specification, dict)
        and specification.get("git")
        == "https://github.com/nautechsystems/nautilus_trader.git"
        and isinstance(specification.get("rev"), str)
    }
    if len(revisions) != 1:
        raise AssertionError(
            "root Cargo.toml must expose exactly one canonical NT test-fixture revision"
        )
    return revisions.pop()


EXPECTED_NT_REV = fixture_nt_revision()
OLD_NT_REV = "0000000000000000000000000000000000000000"
REQUIRED_PIN_SURFACES = (
    "Cargo.toml",
    "Cargo.lock",
    "crates/backtesting-vertical-slice/Cargo.toml",
    "crates/backtesting-vertical-slice/Cargo.lock",
    "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md",
    "docs/bolt-v3/2026-04-25-bolt-v3-schema.md",
    "docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md",
    "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml",
    "tests/fixtures/nt_polymarket_query_post_order_params_d81be0bc.txt",
)


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bolt_v3_boundary_evidence", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write(root: Path, rel: str, text: str | bytes) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(text, bytes):
        path.write_bytes(text)
    else:
        path.write_text(text, encoding="utf-8")


def clean_files(root: Path) -> None:
    write(
        root,
        "Cargo.toml",
        "[dependencies]\n"
        f'nautilus-binance = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}\n'
        f'nautilus-network = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}\n'
    )
    write(
        root,
        "Cargo.lock",
        "version = 4\n"
        "[[package]]\n"
        'name = "nautilus-network"\n'
        'version = "0.59.0"\n'
        f'source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev={EXPECTED_NT_REV}#{EXPECTED_NT_REV}"\n',
    )
    write(
        root,
        "crates/backtesting-vertical-slice/Cargo.toml",
        "[dependencies]\n"
        f'nautilus-model = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}\n',
    )
    write(
        root,
        "crates/backtesting-vertical-slice/Cargo.lock",
        "version = 4\n"
        "[[package]]\n"
        'name = "nautilus-model"\n'
        'version = "0.59.0"\n'
        f'source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev={EXPECTED_NT_REV}#{EXPECTED_NT_REV}"\n',
    )
    write(
        root,
        "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md",
        "### 9.3 Common required fields\n"
        f"  - current value: `{EXPECTED_NT_REV}`\n"
        "### 11.5 NautilusTrader pin governance\n"
        f"The live Binance Spot SBE quote boundary is owned by NautilusTrader revision `{EXPECTED_NT_REV}`.\n"
        "`BinanceSpotDataClient::handle_ws_message`\n"
        "`handle_ws_message_uses_clock_timestamp_for_sbe_bbo_ts_init`\n"
        "`decode_market_data`\n"
        "`parse_trades_event`\n"
        "`parse_bbo_event`\n"
        "`parse_depth_snapshot`\n"
        "`parse_depth_diff`\n"
        "`RealizedVolatilityObservation`\n"
        "`StrategySignalObservation`\n"
        "## 13. CLOB V2 Readiness Gate\n"
        "Current status: this branch pins the official NautilusTrader repository at\n"
        f"merge commit `{EXPECTED_NT_REV}` for upstream PR #4474.\n",
    )
    write(
        root,
        "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml",
        f'nautilus_trader_revision: "{EXPECTED_NT_REV}"\n',
    )
    write(
        root,
        "docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md",
        "Last full NT doctrine audit rev: "
        "`56a438216442f079edf322a39cdc0d9e655ba6d8`\n"
        f"Last NT pin compatibility verified rev: `{EXPECTED_NT_REV}`\n",
    )
    write(
        root,
        "docs/bolt-v3/2026-04-25-bolt-v3-schema.md",
        f"- `qsize` must equal the pinned NT `LiveDataEngineConfig::default().qsize` value, verified as `100000` at pinned NT rev `{EXPECTED_NT_REV}`\n"
        f"| `qsize` | must equal the pinned NT `LiveDataEngineConfig::default().qsize` value, verified as `100000` at pinned NT rev `{EXPECTED_NT_REV}` | `LiveDataEngineConfig.qsize` |\n"
        f"- `qsize` must equal the pinned NT `LiveExecEngineConfig::default().qsize` value, verified as `100000` at pinned NT rev `{EXPECTED_NT_REV}`\n"
        f"| `qsize` | must equal the pinned NT `LiveExecEngineConfig::default().qsize` value, verified as `100000` at pinned NT rev `{EXPECTED_NT_REV}` | `LiveExecEngineConfig.qsize` |\n"
        f"- must equal the pinned NT `LiveRiskEngineConfig::default().qsize` value, verified as `100000` at pinned NT rev `{EXPECTED_NT_REV}`\n"
        f"Historical evidence only: `{OLD_NT_REV}`\n",
    )
    write(
        root,
        "tests/fixtures/nt_polymarket_query_post_order_params_d81be0bc.txt",
        "Source: NautilusTrader\n"
        f"Revision: {EXPECTED_NT_REV}\n"
        "Path: crates/adapters/polymarket/src/http/query.rs\n",
    )
    write(
        root,
        "src/bolt_v3_providers/boundary_registry.rs",
        """
pub const AWS_SSM_SECRET_SOURCE_ADAPTER_ID: &str = stringify!(AwsSsmSecretSource);
pub const IMDS_METADATA_ADAPTER_ID: &str = stringify!(Imdsv2HostFactsSource);
pub enum BoundaryEvidenceClass {
    WebSocketFrame,
    ImdsMetadata,
    AwsSdkResponse,
    HttpResponseBody,
}
pub enum BoundaryFeeder {
    ReferenceCurrentPriceHealth,
    ReferenceLiveProbe,
    RealizedVolatilityObservation,
    StrategySignalObservation,
    DeployTargetHostFacts,
    SecretResolution,
    PolymarketVenueTruthRuntime,
}
pub struct BoundaryRegistryEntry {
    pub adapter_id: &'static str,
    pub class: BoundaryEvidenceClass,
    pub feeder: BoundaryFeeder,
}
pub const BOUNDARY_REGISTRY: &[BoundaryRegistryEntry] = &[
    BoundaryRegistryEntry { adapter_id: chainlink_reference::KEY, class: BoundaryEvidenceClass::WebSocketFrame, feeder: BoundaryFeeder::ReferenceCurrentPriceHealth },
    BoundaryRegistryEntry { adapter_id: polyresearch::KEY, class: BoundaryEvidenceClass::WebSocketFrame, feeder: BoundaryFeeder::ReferenceCurrentPriceHealth },
    BoundaryRegistryEntry { adapter_id: chainlink_reference::KEY, class: BoundaryEvidenceClass::WebSocketFrame, feeder: BoundaryFeeder::ReferenceLiveProbe },
    BoundaryRegistryEntry { adapter_id: polyresearch::KEY, class: BoundaryEvidenceClass::WebSocketFrame, feeder: BoundaryFeeder::ReferenceLiveProbe },
    BoundaryRegistryEntry { adapter_id: BINANCE_SPOT_SBE_ADAPTER_ID, class: BoundaryEvidenceClass::WebSocketFrame, feeder: BoundaryFeeder::RealizedVolatilityObservation },
    BoundaryRegistryEntry { adapter_id: BINANCE_SPOT_SBE_ADAPTER_ID, class: BoundaryEvidenceClass::WebSocketFrame, feeder: BoundaryFeeder::StrategySignalObservation },
    BoundaryRegistryEntry { adapter_id: IMDS_METADATA_ADAPTER_ID, class: BoundaryEvidenceClass::ImdsMetadata, feeder: BoundaryFeeder::DeployTargetHostFacts },
    BoundaryRegistryEntry { adapter_id: AWS_SSM_SECRET_SOURCE_ADAPTER_ID, class: BoundaryEvidenceClass::AwsSdkResponse, feeder: BoundaryFeeder::SecretResolution },
    BoundaryRegistryEntry { adapter_id: polymarket::KEY, class: BoundaryEvidenceClass::HttpResponseBody, feeder: BoundaryFeeder::PolymarketVenueTruthRuntime },
];
""",
    )
    write(
        root,
        "src/bolt_v3_providers/mod.rs",
        """
pub enum ReferencePriceIdentifierKind {
    InstrumentId,
    Symbol,
}
pub struct ReferencePriceProviderMetadata {
    pub provider_key: &'static str,
    pub client_venue_key: &'static str,
    pub identifier_kind: ReferencePriceIdentifierKind,
    pub supported_assets: &'static [&'static str],
}
pub const REFERENCE_PRICE_PROVIDER_METADATA: &[ReferencePriceProviderMetadata] = &[
    ReferencePriceProviderMetadata {
        provider_key: chainlink_reference::REFERENCE_PRICE_PROVIDER_KEY,
        client_venue_key: chainlink_reference::KEY,
        identifier_kind: ReferencePriceIdentifierKind::InstrumentId,
        supported_assets: &[],
    },
    ReferencePriceProviderMetadata {
        provider_key: polyresearch::REFERENCE_PRICE_PROVIDER_KEY,
        client_venue_key: polyresearch::KEY,
        identifier_kind: ReferencePriceIdentifierKind::Symbol,
        supported_assets: &[],
    },
];
fn validate_reference_live_probe_block() {
    chainlink_reference::KEY;
    polyresearch::KEY;
}
const PROVIDER_BINDINGS: &[ProviderBinding] = &[
    ProviderBinding { key: chainlink_reference::KEY },
    ProviderBinding { key: polyresearch::KEY },
];
""",
    )
    write(
        root,
        "src/bolt_v3_wire_boundary.rs",
        """
fn connect_websocket() {
    WebSocketClient::connect();
}
""",
    )
    write(root, "src/bolt_v3_providers/polyresearch.rs", "pub const KEY: &str = \"POLY\";\n")
    write(root, "src/secrets.rs", "use aws_sdk_ssm::{Client as SsmClient};\n")
    write(
        root,
        "src/main.rs",
        """
fn launch() {
    Box::new(Imdsv2HostFactsSource::new());
    deploy_target_status(config_root, &Imdsv2HostFactsSource::new());
}
""",
    )
    write(
        root,
        "ci/bolt-v3-boundary-exemptions.toml",
        """
schema_version = 1
[[evidence_deferred]]
adapter_id = "Imdsv2HostFactsSource"
class = "ImdsMetadata"
feeder = "DeployTargetHostFacts"
issue = 991
expires_on = "2026-07-31"
reason = "test"
[[evidence_deferred]]
adapter_id = "AwsSsmSecretSource"
class = "AwsSdkResponse"
feeder = "SecretResolution"
issue = 991
expires_on = "2026-07-31"
reason = "test"
[[evidence_deferred]]
adapter_id = "POLYMARKET"
class = "HttpResponseBody"
feeder = "PolymarketVenueTruthRuntime"
issue = 874
expires_on = "2026-08-31"
reason = "test"
""",
    )
    write(
        root,
        "ci/rust-verification.toml",
        """
[local_lane_policy]
cheap_lane_labels = ["test_verify_bolt_v3_boundary_evidence.py", "verify_bolt_v3_boundary_evidence.py"]
""",
    )
    write(
        root,
        ".github/workflows/ci.yml",
        """
on:
  workflow_dispatch:
    inputs:
      capture_reference_boundary_fixture: {}
      credential_ssm_gate: {}
jobs:
  source-fence:
    steps:
      - name: source-fence
        env:
          GITHUB_TOKEN: ${{ github.token }}
          GITHUB_REPOSITORY: ${{ github.repository }}
        run: just source-fence
  capture:
    steps:
      - env:
          GH_TOKEN: ${{ github.token }}
        run: |
          check_suite_id="$(gh api "repos/${{ github.repository }}/actions/runs/${{ github.run_id }}" --jq '.check_suite_id')"
          echo "check_suite_id=$check_suite_id" >> "$GITHUB_OUTPUT"
      - run: ops capture-reference-boundary-fixture --root-config config/root.toml
          --check-suite-id "${{ steps.provenance.outputs.check_suite_id }}"
      - run: echo CREDENTIAL-SSM credential_ssm_gate
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a
  capture-gate:
    needs: [capture]
    steps:
      - run: echo capture-gate
""",
    )
    initialize_fixture_repo(root)


def initialize_fixture_repo(root: Path) -> None:
    init_fixture_repo(root, "-q")
    subprocess.run(repo_git_command("config", "user.name", "Boundary Test"), cwd=root, check=True)
    subprocess.run(repo_git_command("config", "user.email", "boundary-test@example.invalid"), cwd=root, check=True)
    subprocess.run(repo_git_command("add", "."), cwd=root, check=True)
    subprocess.run(
        repo_git_command("commit", "--no-verify", "-q", "-m", "seed workflow"),
        cwd=root,
        check=True,
    )


def scan_temp(mutator=None, today: dt.date = dt.date(2026, 6, 26)) -> list[str]:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        clean_files(root)
        if mutator is not None:
            mutator(root)
        return verifier.scan_root(root, today=today)


def assert_finding(findings: list[str], needle: str) -> None:
    if not any(needle in finding for finding in findings):
        raise AssertionError(f"missing finding containing {needle!r}: {findings}")


def test_pin_census_rejects_each_mismatched_surface() -> None:
    for surface in REQUIRED_PIN_SURFACES[1:]:
        def mutate(root: Path, surface: str = surface) -> None:
            path = root / surface
            path.write_text(
                path.read_text(encoding="utf-8").replace(EXPECTED_NT_REV, OLD_NT_REV),
                encoding="utf-8",
            )

        assert_finding(scan_temp(mutate), f"{surface}: NautilusTrader pin census")


def test_source_guard_rejects_personal_repository_without_retaining_literal() -> None:
    forbidden = "https://github.com/" + "seungpyoson/" + "nautilus_trader.git"

    def mutate(root: Path) -> None:
        write(root, "forbidden-source.txt", forbidden)
        subprocess.run(repo_git_command("add", "forbidden-source.txt"), cwd=root, check=True)

    assert_finding(scan_temp(mutate), "source guard forbids the personal repository")


def test_pin_census_derives_revision_from_root_manifest() -> None:
    alternate_revision = "2" * 40

    def mutate(root: Path) -> None:
        for surface in REQUIRED_PIN_SURFACES:
            path = root / surface
            path.write_text(
                path.read_text(encoding="utf-8").replace(
                    EXPECTED_NT_REV, alternate_revision
                ),
                encoding="utf-8",
            )

    assert scan_temp(mutate) == []


def test_pin_census_uses_root_revision_to_reject_stale_dependents() -> None:
    alternate_revision = "2" * 40

    def mutate(root: Path) -> None:
        manifest = root / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                EXPECTED_NT_REV, alternate_revision
            ),
            encoding="utf-8",
        )

    findings = scan_temp(mutate)
    assert not any(
        finding.startswith("Cargo.toml: NautilusTrader pin census")
        for finding in findings
    ), findings
    assert_finding(findings, "Cargo.lock: NautilusTrader pin census")
    assert_finding(
        findings,
        "crates/backtesting-vertical-slice/Cargo.toml: NautilusTrader pin census",
    )


def test_pin_census_rejects_ambiguous_binance_revision_in_root_manifest() -> None:
    alternate_revision = "2" * 40

    def mutate(root: Path) -> None:
        manifest = root / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                f'nautilus-binance = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}',
                f'nautilus-binance = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{alternate_revision}" }}',
            ),
            encoding="utf-8",
        )

    assert_finding(
        scan_temp(mutate),
        "Cargo.toml: NautilusTrader pin census canonical root manifest must declare "
        "exactly one shared immutable",
    )


def test_pin_census_rejects_non_commit_root_revision() -> None:
    def mutate(root: Path) -> None:
        manifest = root / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                EXPECTED_NT_REV, "not-an-immutable-commit"
            ),
            encoding="utf-8",
        )

    assert_finding(
        scan_temp(mutate),
        "Cargo.toml: NautilusTrader pin census canonical root manifest must declare "
        "exactly one shared immutable",
    )


def test_pin_census_checks_binance_dependency_against_derived_revision() -> None:
    def mutate(root: Path) -> None:
        manifest = root / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                f'nautilus-binance = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}',
                'nautilus-binance = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "not-an-immutable-commit" }',
            ),
            encoding="utf-8",
        )

    assert_finding(
        scan_temp(mutate),
        "Cargo.toml: NautilusTrader pin census dependencies.nautilus-binance "
        "must use",
    )


def test_doctrine_pin_census_keeps_full_audit_revision_separate() -> None:
    def mutate(root: Path) -> None:
        path = root / "docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md"
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                "56a438216442f079edf322a39cdc0d9e655ba6d8",
                "2" * 40,
            ),
            encoding="utf-8",
        )

    findings = scan_temp(mutate)
    doctrine_findings = [
        finding
        for finding in findings
        if finding.startswith(
            "docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md: "
            "NautilusTrader pin census"
        )
    ]
    assert doctrine_findings == []


def test_schema_pin_census_rejects_duplicate_governed_claim_in_decoy_section() -> None:
    def mutate(root: Path) -> None:
        path = root / "docs/bolt-v3/2026-04-25-bolt-v3-schema.md"
        governed_claim = (
            "- `qsize` must equal the pinned NT "
            "`LiveDataEngineConfig::default().qsize` value, verified as `100000` "
            f"at pinned NT rev `{EXPECTED_NT_REV}`"
        )
        path.write_text(
            path.read_text(encoding="utf-8")
            + f"\n## Historical decoy\n\n{governed_claim}\n",
            encoding="utf-8",
        )

    assert_finding(scan_temp(mutate), "2026-04-25-bolt-v3-schema.md: NautilusTrader pin census")


def test_schema_pin_census_ignores_unanchored_historical_revision() -> None:
    def mutate(root: Path) -> None:
        path = root / "docs/bolt-v3/2026-04-25-bolt-v3-schema.md"
        path.write_text(
            path.read_text(encoding="utf-8")
            + f"\nHistorical source note: old NT rev `{OLD_NT_REV}`.\n",
            encoding="utf-8",
        )

    findings = scan_temp(mutate)
    schema_findings = [
        finding
        for finding in findings
        if finding.startswith(
            "docs/bolt-v3/2026-04-25-bolt-v3-schema.md: NautilusTrader pin census"
        )
    ]
    assert schema_findings == []


def test_pin_census_reports_one_pin_finding_for_each_missing_required_surface() -> None:
    for surface in REQUIRED_PIN_SURFACES:
        def mutate(root: Path, surface: str = surface) -> None:
            if surface.endswith(("Cargo.toml", "Cargo.lock")):
                subprocess.run(repo_git_command("add", surface), cwd=root, check=True)
            (root / surface).unlink()

        findings = scan_temp(mutate)
        matches = [
            finding
            for finding in findings
            if finding.startswith(
                f"{surface}: NautilusTrader pin census required pin surface could not be read:"
            )
        ]
        if len(matches) != 1:
            raise AssertionError((surface, findings))


def test_manifest_pin_census_accepts_order_multiline_and_dependency_scopes() -> None:
    manifest = f'''
[dependencies]
nautilus-binance = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}
nautilus-common = {{ rev = "{EXPECTED_NT_REV}", git = "https://github.com/nautechsystems/nautilus_trader.git" }}

[dev-dependencies.nautilus-core]
rev = "{EXPECTED_NT_REV}"
git = "https://github.com/nautechsystems/nautilus_trader.git"

[build-dependencies]
nautilus-model = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}

[target.'cfg(unix)'.dependencies.nautilus-network]
git = "https://github.com/nautechsystems/nautilus_trader.git"
rev = "{EXPECTED_NT_REV}"
'''

    def mutate(root: Path) -> None:
        write(root, "Cargo.toml", manifest)
        write(root, "crates/backtesting-vertical-slice/Cargo.toml", manifest)

    assert scan_temp(mutate) == []


def test_manifest_pin_census_rejects_hidden_mixed_and_malformed_sources() -> None:
    personal_source = "https://github.com/" + "seungpyoson/" + "nautilus_trader.git"
    cases = {
        "reordered inline old pin": f'{{ rev = "{OLD_NT_REV}", git = "https://github.com/nautechsystems/nautilus_trader.git" }}',
        "alternate source": f'{{ git = "{personal_source}", rev = "{EXPECTED_NT_REV}" }}',
        "unpinned source": '{ git = "https://github.com/nautechsystems/nautilus_trader.git" }',
        "branch source": '{ git = "https://github.com/nautechsystems/nautilus_trader.git", branch = "develop" }',
    }
    for label, bad_dependency in cases.items():
        manifest = (
            "[dependencies]\n"
            f'nautilus-common = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}\n'
            f"nautilus-core = {bad_dependency}\n"
        )

        def mutate(root: Path, manifest: str = manifest) -> None:
            write(root, "Cargo.toml", manifest)

        findings = scan_temp(mutate)
        assert_finding(findings, "Cargo.toml: NautilusTrader pin census")
        if (
            label not in str(findings)
            and "nautilus-core" not in str(findings)
            and "exactly one shared immutable" not in str(findings)
        ):
            raise AssertionError((label, findings))


def test_manifest_pin_census_rejects_target_dev_and_build_mismatches() -> None:
    scopes = (
        "[dev-dependencies]",
        "[build-dependencies]",
        "[target.'cfg(unix)'.dependencies]",
        "[target.'cfg(unix)'.dev-dependencies]",
        "[target.'cfg(unix)'.build-dependencies]",
    )
    for scope in scopes:
        manifest = (
            "[dependencies]\n"
            f'nautilus-common = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}\n\n'
            f"{scope}\n"
            f'nautilus-core = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{OLD_NT_REV}" }}\n'
        )

        def mutate(root: Path, manifest: str = manifest) -> None:
            write(root, "Cargo.toml", manifest)

        assert_finding(scan_temp(mutate), "Cargo.toml: NautilusTrader pin census")


def test_manifest_pin_census_rejects_multiline_old_pin_with_valid_decoy() -> None:
    manifest = f'''
[dependencies]
nautilus-common = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}

[dependencies.nautilus-core]
git = "https://github.com/nautechsystems/nautilus_trader.git"
rev = "{OLD_NT_REV}"
'''

    def mutate(root: Path) -> None:
        write(root, "Cargo.toml", manifest)

    assert_finding(scan_temp(mutate), "Cargo.toml: NautilusTrader pin census")


def test_manifest_pin_census_rejects_every_nt_override_form() -> None:
    overrides = (
        """
[patch.crates-io]
nautilus-model = { path = "vendor/nautilus-model" }
""",
        """
[patch.crates-io]
nt-model = { package = "nautilus-model", path = "vendor/nautilus-model" }
""",
        """
[patch."https://github.com/nautechsystems/nautilus_trader.git"]
model-fork = { path = "vendor/nautilus-model" }
""",
        f"""
[patch.crates-io]
model-fork = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}
""",
        """
[replace]
"nautilus-model:0.59.0" = { path = "vendor/nautilus-model" }
""",
        """
[replace]
"nautilus-model@0.59.0" = { path = "vendor/nautilus-model" }
""",
        """
[replace]
"https://github.com/nautechsystems/nautilus_trader.git#nautilus-model@0.59.0" = { path = "vendor/nautilus-model" }
""",
    )
    for override in overrides:
        def mutate(root: Path, override: str = override) -> None:
            manifest = root / "Cargo.toml"
            manifest.write_text(
                manifest.read_text(encoding="utf-8") + override,
                encoding="utf-8",
            )

        assert_finding(
            scan_temp(mutate),
            "Cargo.toml: NautilusTrader pin census forbids NT-relevant Cargo override",
        )


def test_manifest_pin_census_ignores_package_metadata_dependency_decoys() -> None:
    def mutate(root: Path) -> None:
        manifest = root / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8")
            + f'''
[package.metadata.dependencies]
nautilus-binance = {{ package = "parser-shim", path = "parser-shim" }}
old-nt-decoy = {{ package = "nautilus-binance", git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{OLD_NT_REV}" }}

[package.metadata.target.'cfg(unix)'.dev-dependencies]
nautilus_binance = {{ package = "parser-shim", path = "parser-shim" }}
''',
            encoding="utf-8",
        )

    assert scan_temp(mutate) == []


def test_manifest_pin_census_governs_actual_cargo_dependency_scopes() -> None:
    scopes = (
        "[dependencies]",
        "[dev-dependencies]",
        "[build-dependencies]",
        "[workspace.dependencies]",
        "[target.'cfg(unix)'.dependencies]",
        "[target.'cfg(unix)'.dev-dependencies]",
        "[target.'cfg(unix)'.build-dependencies]",
    )
    for index, scope in enumerate(scopes):
        def mutate(root: Path, index: int = index, scope: str = scope) -> None:
            surface = f"tools/actual-scope-{index}/Cargo.toml"
            write(
                root,
                surface,
                '[package]\nname = "actual-scope"\nversion = "0.1.0"\n\n'
                + scope
                + "\n"
                + f'nt-binance = {{ package = "nautilus-binance", git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{OLD_NT_REV}" }}\n',
            )
            subprocess.run(repo_git_command("add", surface), cwd=root, check=True)

        assert_finding(
            scan_temp(mutate),
            f"tools/actual-scope-{index}/Cargo.toml: NautilusTrader pin census",
        )


def test_pin_census_discovers_tracked_override_only_standalone_workspace() -> None:
    override_manifests = (
        """
[package]
name = "patch-only"
version = "0.1.0"

[patch.crates-io]
nautilus-model = { path = "../../vendor/nautilus-model" }
""",
        """
[package]
name = "replace-only"
version = "0.1.0"

[replace]
"nautilus-model:0.59.0" = { path = "../../vendor/nautilus-model" }
""",
    )
    for index, manifest_text in enumerate(override_manifests):
        def mutate(
            root: Path,
            index: int = index,
            manifest_text: str = manifest_text,
        ) -> None:
            manifest = f"crates/override-only-{index}/Cargo.toml"
            write(root, manifest, manifest_text)
            subprocess.run(repo_git_command("add", manifest), cwd=root, check=True)

        assert_finding(
            scan_temp(mutate),
            f"crates/override-only-{index}/Cargo.toml: NautilusTrader pin census "
            "forbids NT-relevant Cargo override",
        )


def test_pin_census_ignores_unrelated_cargo_overrides() -> None:
    def mutate(root: Path) -> None:
        manifest = root / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8")
            + """
[patch.crates-io]
serde = { path = "vendor/serde" }

[replace]
"itoa:1.0.0" = { path = "vendor/itoa" }
""",
            encoding="utf-8",
        )

    assert scan_temp(mutate) == []


def test_pin_census_uses_tracked_lockfile_as_override_backstop() -> None:
    def mutate(root: Path) -> None:
        manifest = "crates/lock-backstop/Cargo.toml"
        lockfile = "crates/lock-backstop/Cargo.lock"
        write(
            root,
            manifest,
            "[package]\nname = \"lock-backstop\"\nversion = \"0.1.0\"\n",
        )
        write(
            root,
            lockfile,
            "version = 4\n"
            "[[package]]\n"
            'name = "nautilus-model"\n'
            'version = "0.59.0"\n'
            f'source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev={OLD_NT_REV}#{OLD_NT_REV}"\n',
        )
        subprocess.run(
            repo_git_command("add", manifest, lockfile),
            cwd=root,
            check=True,
        )

    assert_finding(
        scan_temp(mutate),
        "crates/lock-backstop/Cargo.lock: NautilusTrader pin census",
    )


def test_pin_census_rejects_tracked_new_standalone_workspace_with_stale_nt_pin() -> None:
    def mutate(root: Path) -> None:
        manifest = "crates/stale-standalone/Cargo.toml"
        lockfile = "crates/stale-standalone/Cargo.lock"
        write(
            root,
            manifest,
            "[dependencies]\n"
            f'nautilus-model = {{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{OLD_NT_REV}" }}\n',
        )
        write(
            root,
            lockfile,
            "version = 4\n"
            "[[package]]\n"
            'name = "nautilus-model"\n'
            'version = "0.59.0"\n'
            f'source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev={OLD_NT_REV}#{OLD_NT_REV}"\n',
        )
        subprocess.run(
            repo_git_command("add", manifest, lockfile),
            cwd=root,
            check=True,
        )

    findings = scan_temp(mutate)
    assert_finding(
        findings,
        "crates/stale-standalone/Cargo.toml: NautilusTrader pin census",
    )
    assert_finding(
        findings,
        "crates/stale-standalone/Cargo.lock: NautilusTrader pin census",
    )


def test_pin_census_ignores_tracked_non_nt_cargo_surfaces() -> None:
    def mutate(root: Path) -> None:
        manifest = "tools/local-helper/Cargo.toml"
        lockfile = "tools/local-helper/Cargo.lock"
        write(
            root,
            manifest,
            "[package]\nname = \"local-helper\"\nversion = \"0.1.0\"\n"
            "[dependencies]\nserde = \"1\"\n",
        )
        write(
            root,
            lockfile,
            "version = 4\n"
            "[[package]]\n"
            'name = "serde"\n'
            'version = "1.0.0"\n',
        )
        subprocess.run(
            repo_git_command("add", manifest, lockfile),
            cwd=root,
            check=True,
        )

    assert scan_temp(mutate) == []


def test_pin_census_accepts_tracked_workspace_inherited_nt_dependency() -> None:
    def mutate(root: Path) -> None:
        manifest = "crates/inherited-member/Cargo.toml"
        write(
            root,
            manifest,
            "[package]\nname = \"inherited-member\"\nversion = \"0.1.0\"\n"
            "[dependencies]\nnautilus-model = { workspace = true }\n",
        )
        subprocess.run(repo_git_command("add", manifest), cwd=root, check=True)

    assert scan_temp(mutate) == []


def test_pin_census_rejects_workspace_inheritance_on_canonical_pin_roots() -> None:
    for manifest in (
        "Cargo.toml",
        "crates/backtesting-vertical-slice/Cargo.toml",
    ):
        def mutate(root: Path, manifest: str = manifest) -> None:
            path = root / manifest
            path.write_text(
                path.read_text(encoding="utf-8").replace(
                    f'{{ git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "{EXPECTED_NT_REV}" }}',
                    "{ workspace = true }",
                ),
                encoding="utf-8",
            )

        assert_finding(
            scan_temp(mutate),
            f"{manifest}: NautilusTrader pin census",
        )


def test_lock_pin_census_accepts_reordered_package_fields() -> None:
    source = (
        "git+https://github.com/nautechsystems/nautilus_trader.git"
        f"?rev={EXPECTED_NT_REV}#{EXPECTED_NT_REV}"
    )
    lock = f'''
version = 4
[[package]]
source = "{source}"
version = "0.59.0"
name = "nautilus-common"
'''

    def mutate(root: Path) -> None:
        write(root, "Cargo.lock", lock)
        write(root, "crates/backtesting-vertical-slice/Cargo.lock", lock)

    assert scan_temp(mutate) == []


def test_lock_pin_census_rejects_hidden_mixed_and_malformed_sources() -> None:
    canonical = (
        "git+https://github.com/nautechsystems/nautilus_trader.git"
        f"?rev={EXPECTED_NT_REV}#{EXPECTED_NT_REV}"
    )
    personal_source = "git+https://github.com/" + "seungpyoson/" + "nautilus_trader.git"
    cases = {
        "alternate source": personal_source
        + f"?rev={EXPECTED_NT_REV}#{EXPECTED_NT_REV}",
        "missing rev": (
            "git+https://github.com/nautechsystems/nautilus_trader.git"
            f"#{EXPECTED_NT_REV}"
        ),
        "old rev": (
            "git+https://github.com/nautechsystems/nautilus_trader.git"
            f"?rev={OLD_NT_REV}#{OLD_NT_REV}"
        ),
        "wrong commit": (
            "git+https://github.com/nautechsystems/nautilus_trader.git"
            f"?rev={EXPECTED_NT_REV}#{OLD_NT_REV}"
        ),
    }
    for label, bad_source in cases.items():
        lock = f'''
version = 4
[[package]]
name = "nautilus-common"
version = "0.59.0"
source = "{canonical}"
[[package]]
name = "nautilus-core"
version = "0.59.0"
source = "{bad_source}"
'''

        def mutate(root: Path, lock: str = lock) -> None:
            write(root, "Cargo.lock", lock)

        findings = scan_temp(mutate)
        assert_finding(findings, "Cargo.lock: NautilusTrader pin census")
        if label not in str(findings) and "nautilus-core" not in str(findings):
            raise AssertionError((label, findings))


def test_binance_registry_row_alone_cannot_masquerade_as_sha_provenance() -> None:
    def mutate(root: Path) -> None:
        path = root / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"
        path.write_text(
            path.read_text(encoding="utf-8").replace("`parse_bbo_event`\n", ""),
            encoding="utf-8",
        )

    assert_finding(
        scan_temp(mutate),
        "### 11.5 NautilusTrader pin governance missing parse_bbo_event",
    )


def test_pin_census_rejects_one_conflicting_runtime_contract_occurrence() -> None:
    def mutate(root: Path) -> None:
        path = root / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"
        text = path.read_text(encoding="utf-8")
        path.write_text(
            text.replace(
                f"merge commit `{EXPECTED_NT_REV}` for upstream PR #4474.",
                f"merge commit `{OLD_NT_REV}` for upstream PR #4474.",
            ),
            encoding="utf-8",
        )

    assert_finding(scan_temp(mutate), "runtime-contracts.md: NautilusTrader pin census")


def test_text_pin_census_rejects_comment_and_expression_decoys() -> None:
    def mutate(root: Path) -> None:
        naming = root / "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml"
        naming.write_text(
            f'# nautilus_trader_revision: "{EXPECTED_NT_REV}"\n'
            f'nautilus_trader_revision: "{OLD_NT_REV}"\n',
            encoding="utf-8",
        )
    findings = scan_temp(mutate)
    assert_finding(findings, "nt-owned-name-audit.yaml: NautilusTrader pin census")


def test_runtime_contract_pin_census_rejects_wrong_section_decoy() -> None:
    def mutate(root: Path) -> None:
        contract = root / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"
        text = contract.read_text(encoding="utf-8")
        text = text.replace(f"  - current value: `{EXPECTED_NT_REV}`\n", "")
        contract.write_text(
            text + f"\n## Decoy\n\n  - current value: `{EXPECTED_NT_REV}`\n",
            encoding="utf-8",
        )

    assert_finding(scan_temp(mutate), "runtime-contracts.md: NautilusTrader pin census")


def test_runtime_contract_requires_one_pin_per_owner_section() -> None:
    def mutate(root: Path) -> None:
        contract = root / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"
        text = contract.read_text(encoding="utf-8")
        text = text.replace(f"  - current value: `{EXPECTED_NT_REV}`\n", "")
        owner_pin = (
            "The live Binance Spot SBE quote boundary is owned by NautilusTrader revision "
            f"`{EXPECTED_NT_REV}`."
        )
        contract.write_text(
            text.replace(owner_pin, f"{owner_pin}\n{owner_pin}"),
            encoding="utf-8",
        )

    assert_finding(scan_temp(mutate), "### 9.3 Common required fields")
    assert_finding(scan_temp(mutate), "### 11.5 NautilusTrader pin governance")


def test_runtime_contract_requires_binance_lineage_inside_owner_section() -> None:
    required = (
        "BinanceSpotDataClient::handle_ws_message",
        "handle_ws_message_uses_clock_timestamp_for_sbe_bbo_ts_init",
        "decode_market_data",
        "parse_trades_event",
        "parse_bbo_event",
        "parse_depth_snapshot",
        "parse_depth_diff",
        "RealizedVolatilityObservation",
        "StrategySignalObservation",
    )

    def mutate(root: Path) -> None:
        contract = root / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"
        text = contract.read_text(encoding="utf-8")
        for index, symbol in enumerate(required):
            text = text.replace(f"`{symbol}`", f"`moved_symbol_{index}`")
        decoy = "\n".join(f"`{symbol}`" for symbol in required)
        contract.write_text(f"{text}\n## Decoy\n\n{decoy}\n", encoding="utf-8")

    findings = scan_temp(mutate)
    for symbol in required:
        assert_finding(findings, f"### 11.5 NautilusTrader pin governance missing {symbol}")


def test_runtime_contract_rejects_duplicate_or_misnamed_owner_heading() -> None:
    mutations = (
        lambda text: text + "\n### 11.5 NautilusTrader pin governance\n",
        lambda text: text.replace(
            "### 11.5 NautilusTrader pin governance",
            "### 11.5 NautilusTrader pins governance",
        ),
    )
    for mutate_text in mutations:
        def mutate(root: Path, mutate_text=mutate_text) -> None:
            contract = root / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"
            contract.write_text(
                mutate_text(contract.read_text(encoding="utf-8")),
                encoding="utf-8",
            )

        assert_finding(scan_temp(mutate), "### 11.5 NautilusTrader pin governance")


def test_runtime_contract_rejects_expected_decoy_with_wrong_owner_value() -> None:
    def mutate(root: Path) -> None:
        contract = root / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"
        text = contract.read_text(encoding="utf-8").replace(
            f"  - current value: `{EXPECTED_NT_REV}`",
            f"  - current value: `{OLD_NT_REV}`",
        )
        contract.write_text(
            text + f"\n## Decoy\n\n  - current value: `{EXPECTED_NT_REV}`\n",
            encoding="utf-8",
        )

    assert_finding(scan_temp(mutate), "### 9.3 Common required fields")


def test_missing_binance_live_quote_feeder_fails_closed() -> None:
    def mutate(root: Path) -> None:
        path = root / "src/bolt_v3_providers/boundary_registry.rs"
        text = path.read_text(encoding="utf-8")
        path.write_text(
            re.sub(
                r"\s*BoundaryRegistryEntry \{ adapter_id: BINANCE_SPOT_SBE_ADAPTER_ID, class: BoundaryEvidenceClass::WebSocketFrame, feeder: BoundaryFeeder::StrategySignalObservation \},",
                "",
                text,
            ),
            encoding="utf-8",
        )

    assert_finding(scan_temp(mutate), "StrategySignalObservation")


def test_empty_wire_boundary_source_set_fails_closed() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        findings = verifier.scan_root(root)

    assert findings == ["Bolt-v3 boundary Rust source files: enforcement set is empty"], findings


def test_planted_unregistered_any_class_fails() -> None:
    def mutate(root: Path) -> None:
        path = root / "src/bolt_v3_providers/boundary_registry.rs"
        text = path.read_text(encoding="utf-8")
        path.write_text(text.replace("BoundaryRegistryEntry { adapter_id: AWS_SSM_SECRET_SOURCE_ADAPTER_ID, class: BoundaryEvidenceClass::AwsSdkResponse, feeder: BoundaryFeeder::SecretResolution },\n", ""), encoding="utf-8")

    assert_finding(scan_temp(mutate), "missing registry entry")


def test_string_literal_non_reference_metadata_provider_without_registry_fails() -> None:
    def mutate(root: Path) -> None:
        path = root / "src/bolt_v3_providers/mod.rs"
        text = path.read_text(encoding="utf-8")
        path.write_text(
            text.replace(
                "];\nfn validate_reference_live_probe_block()",
                '    ReferencePriceProviderMetadata {\n'
                '        provider_key: pyth::REFERENCE_PRICE_PROVIDER_KEY,\n'
                '        client_venue_key: "PYTH_REFERENCE_PRICE",\n'
                '        identifier_kind: ReferencePriceIdentifierKind::Symbol,\n'
                '        supported_assets: &[],\n'
                '    },\n'
                "];\nfn validate_reference_live_probe_block()",
            ),
            encoding="utf-8",
        )

    findings = scan_temp(mutate)
    assert_finding(
        findings,
        "missing registry entry ('\"PYTH_REFERENCE_PRICE\"', 'WebSocketFrame', 'ReferenceCurrentPriceHealth')",
    )
    assert_finding(
        findings,
        "missing registry entry ('\"PYTH_REFERENCE_PRICE\"', 'WebSocketFrame', 'ReferenceLiveProbe')",
    )


def test_stale_registry_row_fails() -> None:
    def mutate(root: Path) -> None:
        path = root / "src/bolt_v3_providers/boundary_registry.rs"
        text = path.read_text(encoding="utf-8")
        path.write_text(
            text.replace(
                "];\n",
                "    BoundaryRegistryEntry { adapter_id: stale_reference::KEY, class: BoundaryEvidenceClass::WebSocketFrame, feeder: BoundaryFeeder::ReferenceLiveProbe },\n];\n",
            ),
            encoding="utf-8",
        )

    assert_finding(scan_temp(mutate), "unexpected registry entry")


def test_capture_workflow_must_not_use_run_id_as_check_suite_id() -> None:
    def mutate(root: Path) -> None:
        path = root / ".github/workflows/ci.yml"
        text = path.read_text(encoding="utf-8")
        expected = '--check-suite-id "${{ steps.provenance.outputs.check_suite_id }}"'
        if expected not in text:
            raise AssertionError("clean workflow fixture missing check_suite_id output binding")
        path.write_text(
            text.replace(expected, '--check-suite-id "${{ github.run_id }}"'),
            encoding="utf-8",
        )

    assert_finding(scan_temp(mutate), "capture provenance must use workflow run check_suite_id")


def test_expired_deferral_fails() -> None:
    assert_finding(scan_temp(today=dt.date(2026, 8, 1)), "expired on 2026-07-31")


def test_temp_root_does_not_verify_github_issue_state_in_actions_env() -> None:
    verifier = load_verifier()
    original_github_actions = os.environ.get("GITHUB_ACTIONS")
    original_issue_state = verifier.github_issue_state

    def fail_issue_state(*_args, **_kwargs):
        raise AssertionError("temp-root self-tests must not call GitHub issue state")

    try:
        os.environ["GITHUB_ACTIONS"] = "true"
        verifier.github_issue_state = fail_issue_state
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            clean_files(root)
            findings: list[str] = []
            verifier.scan_exemption_issue_state(root, findings)
        if findings:
            raise AssertionError(f"unexpected temp-root issue-state findings {findings}")
    finally:
        verifier.github_issue_state = original_issue_state
        if original_github_actions is None:
            os.environ.pop("GITHUB_ACTIONS", None)
        else:
            os.environ["GITHUB_ACTIONS"] = original_github_actions


def test_new_http_feeder_fails_closed() -> None:
    def mutate(root: Path) -> None:
        write(root, "src/new_http.rs", "fn f() { let _ = reqwest::Client::new(); }\n")

    assert_finding(scan_temp(mutate), "HTTP response-body feeder must be registered")


def test_raw_connect_outside_wire_boundary_fails() -> None:
    def mutate(root: Path) -> None:
        write(root, "src/raw_connect.rs", "fn f() { WebSocketClient::connect_url(); }\n")

    assert_finding(scan_temp(mutate), "raw NT wire symbol WebSocketClient connect primitive")


def test_websocket_inner_and_aliased_client_import_outside_wire_boundary_fail() -> None:
    def mutate(root: Path) -> None:
        write(
            root,
            "src/raw_connect.rs",
            """
use nautilus_network::websocket::WebSocketClient as Ws;

async fn f() {
    WebSocketClientInner::connect_url();
    Ws::connect();
}
""",
        )

    findings = scan_temp(mutate)
    assert_finding(findings, "raw NT wire module path nautilus_network::websocket")
    assert_finding(findings, "raw NT wire symbol WebSocketClientInner")


def test_websocket_module_alias_and_renamed_client_outside_wire_boundary_fail() -> None:
    def mutate(root: Path) -> None:
        write(
            root,
            "src/raw_connect.rs",
            """
use nautilus_network::websocket as ws;
use self::ws::WebSocketClient as Foo;

async fn f(config: ws::WebSocketConfig) {
    let _ = Foo::connect(config, None, None, None, vec![], None).await;
}
""",
        )

    findings = scan_temp(mutate)
    assert_finding(findings, "raw NT wire symbol WebSocketClient")


def test_wire_boundary_restricted_visibility_reexport_fails() -> None:
    def mutate(root: Path) -> None:
        with (root / "src/bolt_v3_wire_boundary.rs").open("a", encoding="utf-8") as file:
            file.write("\npub(crate) use nautilus_network::websocket::WebSocketClient;\n")

    assert_finding(scan_temp(mutate), "wire boundary must not re-export raw NT wire symbol WebSocketClient")


def test_wire_boundary_multiline_reexport_fails() -> None:
    def mutate(root: Path) -> None:
        with (root / "src/bolt_v3_wire_boundary.rs").open("a", encoding="utf-8") as file:
            file.write(
                """
pub use nautilus_network::websocket::{
    WebSocketClient,
};
"""
            )

    assert_finding(scan_temp(mutate), "wire boundary must not re-export raw NT wire symbol WebSocketClient")


def test_transport_module_alias_and_renamed_message_outside_wire_boundary_fail() -> None:
    def mutate(root: Path) -> None:
        write(
            root,
            "src/raw_transport.rs",
            """
use nautilus_network::transport as t;
use self::t::Message as M;

fn f(message: M) {
    let _ = message;
}
""",
        )

    assert_finding(scan_temp(mutate), "raw NT wire module path nautilus_network::transport")


def test_crate_alias_websocket_module_outside_wire_boundary_fails() -> None:
    def mutate(root: Path) -> None:
        write(
            root,
            "src/raw_websocket_config.rs",
            """
use nautilus_network as nn;
use nn::websocket as ws;

fn f(config: ws::WebSocketConfig) {
    let _ = config;
}
""",
        )

    assert_finding(scan_temp(mutate), "raw NT wire module path nautilus_network::websocket")


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    tests = [
        value
        for name, value in sorted(globals().items())
        if name.startswith("test_") and callable(value)
    ]
    for test in tests:
        test()
    print(f"OK: {len(tests)} boundary evidence verifier self-tests passed.")
