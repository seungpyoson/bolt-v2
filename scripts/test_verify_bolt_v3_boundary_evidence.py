#!/usr/bin/env python3
"""Self-tests for verify_bolt_v3_boundary_evidence.py."""

from __future__ import annotations

import datetime as dt
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
import zipfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_boundary_evidence.py"
UNRESOLVABLE_SHA = "1" * 40


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


def digest(root: Path, rel: str) -> str:
    return hashlib.sha256((root / rel).read_bytes()).hexdigest()


def clean_files(root: Path) -> None:
    write(
        root,
        "Cargo.toml",
        'nautilus-network = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "6be5a5094716790a8ca2875445fde4fa2586107e" }\n',
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
    DeployTargetHostFacts,
    SecretResolution,
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
    BoundaryRegistryEntry { adapter_id: IMDS_METADATA_ADAPTER_ID, class: BoundaryEvidenceClass::ImdsMetadata, feeder: BoundaryFeeder::DeployTargetHostFacts },
    BoundaryRegistryEntry { adapter_id: AWS_SSM_SECRET_SOURCE_ADAPTER_ID, class: BoundaryEvidenceClass::AwsSdkResponse, feeder: BoundaryFeeder::SecretResolution },
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
    write(
        root,
        "src/bolt_v3_providers/chainlink_reference.rs",
        """
pub const KEY: &str = "CHAINLINK_REFERENCE_PRICE";
fn handler(message: WireMessage) {
    let frame_bytes = match message {
        WireMessage::Text(bytes) | WireMessage::Binary(bytes) => bytes,
        _ => return,
    };
}
#[cfg(test)]
mod tests {
    fn committed_real_capture_frame_decodes_through_production_handler() {}
    fn binary_report_frame_for_active_subscription_emits_custom_reference_update() {}
    fn invalid_utf8_binary_report_frame_emits_no_custom_data() {}
    fn binary_report_frame_through_text_only_handler_emits_no_custom_data() {}
    fn planted_drop_binary_arm_mutation_would_fail_the_binary_observation_test() {}
}
""",
    )
    write(root, "src/bolt_v3_providers/polyresearch.rs", "pub const KEY: &str = \"POLY\";\n")
    write(
        root,
        "src/bolt_v3_reference_price_health.rs",
        """
#[cfg(test)]
mod tests {
    async fn chainlink_binary_loopback_observes_reference_update_through_health_msgbus() {
        prepare_reference_current_price_health_run_with_resolved();
        run_prepared_reference_current_price_health().await;
    }
}
""",
    )
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
""",
    )
    write(
        root,
        "justfile",
        """
source-fence-static-inner:
    python3 scripts/test_verify_bolt_v3_boundary_evidence.py
    python3 scripts/verify_bolt_v3_boundary_evidence.py
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
    write_capture_provenance_config(root)
    write_fixture_and_artifact(root)


def write_capture_provenance_config(root: Path) -> None:
    write(
        root,
        "ci/chainlink-reference-fixture-capture-provenance.toml",
        """
schema_version = 1
[meter]
fingerprint_artifact_prefix = "x-"
fingerprint_workflow = "ci"
[ci_provenance]
schema_version = 1
artifact_name_template = "chainlink-reference-fixture-capture-attempt-{run_attempt}"
workflow_key = "ci"
workflow_name = "CI"
workflow_path = ".github/workflows/ci.yml"
fingerprint_source = "meter"
[ci_provenance.full_ci]
required_jobs = ["capture"]
conditional_jobs = []
conditional_job_outputs = {}
[ci_provenance.full_ci.jobs.capture]
check_name = "capture"
[ci_provenance.deploy]
artifact_name = "chainlink-reference-fixture-capture"
require_source_event = "workflow_dispatch"
require_source_branch = "main"
require_gate_check = false
[ci_provenance.dispatch]
workflow_input = "capture_reference_boundary_fixture"
run_name_default = "Chainlink Reference Fixture Capture"
run_name_full = "Chainlink Reference Fixture Capture [credential-ssm]"
run_name_iteration = "Chainlink Reference Fixture Capture [dry]"
proof_gate_job = "gate"
[ci_provenance.gate_names]
gate_required = "gate"
gate_defer = "gate-deferred"
gate_iteration = "gate-iteration"
gate_noop = "gate-noop"
gate_dispatch_full = "gate-dispatch"
backtester_required = "backtester-gate"
backtester_defer = "backtester-gate-deferred"
backtester_iteration = "backtester-gate-iteration"
backtester_noop = "backtester-gate-noop"
backtester_dispatch_full = "backtester-gate-dispatch"
[ci_provenance.docs]
safe_paths = [
  "AGENTS.md",
  "CLAUDE.md",
  "GEMINI.md",
  "REASONIX.md",
  "LICENSE",
  "SECURITY.md",
  ".github/ISSUE_TEMPLATE/**",
  ".claude/**",
  ".codex/**",
  ".gemini/**",
  ".opencode/**",
  ".pi/**",
  ".specify/**",
]
forbidden_ignored_build_paths = [
  ".claude/rust-verification.toml",
]
non_heavy_required_jobs = ["capture"]
[ci_provenance.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
run_artifacts_per_page = 100
max_lookback_pages = 10
max_lookback_age_seconds = 2592000
[ci_provenance.artifacts]
retention_days = 30
[ci_provenance.policy]
draft_pr_synchronize = "defer"
draft_pr_opened = "defer"
draft_pr_reopened = "defer"
draft_pr_edited = "defer"
converted_to_draft = "defer"
ready_pr = "full"
ready_pr_edited_no_base = "noop"
ready_pr_reopened = "noop"
ready_for_review = "full"
docs = "docs"
workflow_dispatch = "iteration"
workflow_dispatch_full_ci = "full"
main_push = "full"
merge_group = "full"
mergify_temp_pr = "full"
tag = "tag_reuse"
unknown_event = "full"
[ci_provenance.mergify]
temp_pr_head_ref_prefix = "mergify/merge-queue/"
[ci_provenance.policy.override]
force_full_ci = false
ignore_emit_failure = false
""",
    )


def commit_workflow(root: Path) -> str:
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)
    subprocess.run(["git", "config", "user.name", "Boundary Test"], cwd=root, check=True)
    subprocess.run(["git", "config", "user.email", "boundary-test@example.invalid"], cwd=root, check=True)
    subprocess.run(["git", "add", ".github/workflows/ci.yml"], cwd=root, check=True)
    subprocess.run(
        ["git", "commit", "--no-verify", "-q", "-m", "seed workflow"],
        cwd=root,
        check=True,
    )
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return result.stdout.strip()


def write_fixture_and_artifact(root: Path) -> None:
    fixture_rel = "tests/fixtures/bolt_v3/boundary_evidence/chainlink-reference-frame.bin"
    artifact_rel = "tests/fixtures/bolt_v3/boundary_evidence/chainlink-reference-capture.zip"
    write(root, fixture_rel, b"binary frame")
    fixture_sha = digest(root, fixture_rel)
    workflow_digest = digest(root, ".github/workflows/ci.yml")
    capture_sha = commit_workflow(root)
    module = load_verifier()
    config_digest = module.ci_provenance.provenance_config_digest(
        root / "ci/chainlink-reference-fixture-capture-provenance.toml"
    )
    record = {
        "schema_version": 1,
        "kind": "full-ci",
        "repository": "seungpyoson/bolt-v2",
        "workflow_path": ".github/workflows/ci.yml",
        "workflow_digest": workflow_digest,
        "provenance_config_digest": config_digest,
        "head_sha": capture_sha,
        "tested_sha": capture_sha,
        "run_id": 1,
        "run_attempt": 1,
        "check_suite_id": 1,
        "event": "workflow_dispatch",
        "head_branch": "seed-branch",
        "pull_request": {"number": None, "base_sha": None},
        "required_jobs": {"capture": "success"},
        "conditional_jobs": {},
        "nextest_fingerprint": None,
        "created_at": "2026-06-26T00:00:00Z",
        "capture": {
            "record_kind": "chainlink-reference-fixture-capture",
            "adapter_id": "CHAINLINK_REFERENCE_PRICE",
            "client_key": "chainlink_reference",
            "frame_kind": "binary",
            "signature_verified": False,
            "fixture_filename": "chainlink-reference-frame.bin",
            "fixture_sha256": fixture_sha,
            "observed_binary_frames": 1,
            "observed_text_frames": 0,
        },
    }
    artifact_path = root / artifact_rel
    artifact_path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(artifact_path, "w") as archive:
        archive.writestr("ci-provenance.json", json.dumps(record, sort_keys=True))
    write(
        root,
        "tests/fixtures/bolt_v3/boundary_evidence/chainlink-reference-frame.toml",
        f"""
schema_version = 1
adapter_id = "CHAINLINK_REFERENCE_PRICE"
class = "WebSocketFrame"
feeder = "ReferenceCurrentPriceHealth"
frame_kind = "binary"
signature_verified = false
fixture = "chainlink-reference-frame.bin"
fixture_sha256 = "{fixture_sha}"
capture_artifact = "chainlink-reference-capture.zip"
capture_head_sha = "{capture_sha}"
capture_head_branch = "seed-branch"
""",
    )


def mutate_capture_record(root: Path, mutator) -> None:
    artifact_path = root / "tests/fixtures/bolt_v3/boundary_evidence/chainlink-reference-capture.zip"
    with zipfile.ZipFile(artifact_path) as archive:
        record = json.loads(archive.read("ci-provenance.json").decode("utf-8"))
    mutator(record)
    with zipfile.ZipFile(artifact_path, "w") as archive:
        archive.writestr("ci-provenance.json", json.dumps(record, sort_keys=True))


def replace_capture_head_sha(root: Path, sha: str) -> None:
    sidecar = root / "tests/fixtures/bolt_v3/boundary_evidence/chainlink-reference-frame.toml"
    sidecar.write_text(
        re.sub(
            r'capture_head_sha = "[0-9a-f]{40}"',
            f'capture_head_sha = "{sha}"',
            sidecar.read_text(encoding="utf-8"),
        ),
        encoding="utf-8",
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


def test_capture_config_without_workflows_passes_boundary_scan() -> None:
    def assert_no_workflows_table(root: Path) -> None:
        config_path = root / "ci/chainlink-reference-fixture-capture-provenance.toml"
        config = tomllib.loads(config_path.read_text(encoding="utf-8"))
        if "workflows" in config:
            raise AssertionError("capture provenance fixture must not grow a [workflows] table")

    assert scan_temp(assert_no_workflows_table) == []


def test_planted_unregistered_any_class_fails() -> None:
    def mutate(root: Path) -> None:
        path = root / "src/bolt_v3_providers/boundary_registry.rs"
        text = path.read_text(encoding="utf-8")
        path.write_text(text.replace("BoundaryRegistryEntry { adapter_id: AWS_SSM_SECRET_SOURCE_ADAPTER_ID, class: BoundaryEvidenceClass::AwsSdkResponse, feeder: BoundaryFeeder::SecretResolution },\n", ""), encoding="utf-8")

    assert_finding(scan_temp(mutate), "missing registry entry")


def test_parser_only_chainlink_handler_fails() -> None:
    def mutate(root: Path) -> None:
        path = root / "src/bolt_v3_providers/chainlink_reference.rs"
        text = path.read_text(encoding="utf-8")
        path.write_text(text.replace("WireMessage::Text(bytes) | WireMessage::Binary(bytes) => bytes", "message.as_text().unwrap()"), encoding="utf-8")

    findings = scan_temp(mutate)
    assert_finding(findings, "must accept Text and Binary")
    assert_finding(findings, "must not use parser-only as_text")


def test_registered_text_only_handler_fails() -> None:
    def mutate(root: Path) -> None:
        path = root / "src/bolt_v3_providers/chainlink_reference.rs"
        text = path.read_text(encoding="utf-8")
        path.write_text(text.replace("WireMessage::Text(bytes) | WireMessage::Binary(bytes) => bytes", "WireMessage::Text(bytes) => bytes"), encoding="utf-8")

    assert_finding(scan_temp(mutate), "must accept Text and Binary")


def test_missing_committed_real_capture_decode_test_fails() -> None:
    def mutate(root: Path) -> None:
        path = root / "src/bolt_v3_providers/chainlink_reference.rs"
        text = path.read_text(encoding="utf-8")
        path.write_text(
            text.replace(
                "    fn committed_real_capture_frame_decodes_through_production_handler() {}\n",
                "",
            ),
            encoding="utf-8",
        )

    assert_finding(
        scan_temp(mutate),
        "missing test committed_real_capture_frame_decodes_through_production_handler",
    )


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


def test_unbound_invented_fixture_fails() -> None:
    def mutate(root: Path) -> None:
        path = root / "tests/fixtures/bolt_v3/boundary_evidence/chainlink-reference-frame.toml"
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        data["fixture_sha256"] = "0" * 64
        path.write_text("\n".join(f'{key} = "{value}"' for key, value in data.items()) + "\n", encoding="utf-8")

    assert_finding(scan_temp(mutate), "fixture_sha256 does not match")


def test_capture_artifact_uses_tested_sha_workflow_digest_after_workflow_changes() -> None:
    def mutate(root: Path) -> None:
        path = root / ".github/workflows/ci.yml"
        path.write_text(
            path.read_text(encoding="utf-8") + "\n# post-capture workflow lint edit\n",
            encoding="utf-8",
        )

    assert scan_temp(mutate) == []


def test_unresolvable_capture_workflow_sha_fails_closed() -> None:
    def mutate(root: Path) -> None:
        mutate_capture_record(
            root,
            lambda record: record.update(
                {"head_sha": UNRESOLVABLE_SHA, "tested_sha": UNRESOLVABLE_SHA}
            ),
        )
        replace_capture_head_sha(root, UNRESOLVABLE_SHA)

    assert_finding(scan_temp(mutate), "is not resolvable at tested_sha")


def test_unresolvable_capture_workflow_sha_defers_to_remote_resolver_in_ci() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        clean_files(root)
        mutate_capture_record(
            root,
            lambda record: record.update(
                {"head_sha": UNRESOLVABLE_SHA, "tested_sha": UNRESOLVABLE_SHA}
            ),
        )
        replace_capture_head_sha(root, UNRESOLVABLE_SHA)
        artifact = root / "tests/fixtures/bolt_v3/boundary_evidence/chainlink-reference-capture.zip"

        calls: list[tuple[str, str]] = []
        original_remote_context = verifier.remote_fixture_context
        original_resolve = verifier.ci_provenance.resolve_exact_sha_evidence

        def fake_remote_context(scan_root: Path, findings: list[str]):
            if scan_root != root:
                raise AssertionError(f"unexpected scan root {scan_root}")
            return ("seungpyoson/bolt-v2", "token", "999")

        def fake_resolve_exact_sha_evidence(**kwargs):
            calls.append((str(kwargs["requested_sha"]), kwargs["config"].deploy_source_branch))
            with zipfile.ZipFile(artifact) as archive:
                record = json.loads(archive.read("ci-provenance.json").decode("utf-8"))
            return verifier.ci_provenance.ResolvedEvidence(
                run={},
                artifact={},
                record=record,
            )

        try:
            verifier.remote_fixture_context = fake_remote_context
            verifier.ci_provenance.resolve_exact_sha_evidence = fake_resolve_exact_sha_evidence
            findings = verifier.scan_root(root, today=dt.date(2026, 6, 26))
        finally:
            verifier.remote_fixture_context = original_remote_context
            verifier.ci_provenance.resolve_exact_sha_evidence = original_resolve

        if findings:
            raise AssertionError(f"expected remote resolver to validate unresolvable local SHA: {findings}")
        if calls != [(UNRESOLVABLE_SHA, "seed-branch")]:
            raise AssertionError(f"unexpected resolver calls {calls}")


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
