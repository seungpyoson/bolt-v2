#!/usr/bin/env python3
"""Verify RA Gate-0 catalog persistence is wired into the operator path."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ARTIFACT_STORE = Path("crates/backtesting-vertical-slice/src/artifact_store.rs")
CAPABILITY_PROOF = Path("crates/backtesting-vertical-slice/src/nt_catalog_capability.rs")
LIB_PATH = Path("crates/backtesting-vertical-slice/src/lib.rs")
OPERATOR = Path("crates/backtesting-vertical-slice/src/operator.rs")
CONTRACT_TEST = Path("crates/backtesting-vertical-slice/tests/artifact_store_contract.rs")
BTE_CARGO_TOML = Path("crates/backtesting-vertical-slice/Cargo.toml")
JUSTFILE = Path("justfile")
RUN_SPEC = Path(
    "specs/023-nt-research-analytics-platform/reference/"
    "backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
)

CARGO_TOML_REQUIRED = (
    'object_store = { version = "=0.13.2", default-features = false, features = ["aws"] }',
)
ARTIFACT_STORE_REQUIRED = (
    "AHashMap<String, String>",
    "object_store::aws::{AmazonS3, AmazonS3Builder, S3ConditionalPut, S3CopyIfNotExists}",
    "pub struct S3ArtifactStoreConfig",
    "pub s3: S3ArtifactStoreConfig",
    "pub fn build_s3_object_store(&self) -> Result<AmazonS3>",
    "pub fn nt_catalog_storage_options(&self) -> Result<AHashMap<String, String>>",
    "pub fn nt_catalog_storage_options(&self) -> AHashMap<String, String>",
    ".insert(\"region\".to_string(),",
    ".with_bucket_name(",
    ".with_region(",
    ".with_conditional_put(S3ConditionalPut::ETagMatch)",
    ".with_copy_if_not_exists(S3CopyIfNotExists::Multipart)",
    "pub struct CreateOnlyProbeConfig",
    "pub struct CreateOnlyProbeTranscript",
    "copy_source_object_name",
    "copy_dest_object_name",
    "nt_catalog_synthetic_proof",
    "nt_catalog_synthetic_proof_root",
    "subpaths.nt_catalog_synthetic_proof",
    "pub async fn persist_catalog_projection_for_source_binding",
    "pub async fn probe_create_only",
    "duplicate_create_rejected",
    "duplicate_copy_rejected",
    "create_only_probe_uri",
    "copy_if_not_exists",
    "CatalogDispatchConfig",
    ".catalog_root_for(",
    "CreateOnlyArtifactWriter::new",
    ".put_create_idempotent(",
    "fs::read(",
)
CAPABILITY_PROOF_REQUIRED = (
    "pub const NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION",
    "pub const SYNTHETIC_SOURCE_PROOF_ID",
    "pub const REQUIRED_AMBIENT_AWS_CREDENTIAL_ENV_VARS",
    "pub struct NtCatalogCapabilityRunSpec",
    "pub struct NtCatalogCapabilityPlan",
    "pub struct NtCatalogCapabilityProofArtifact",
    "pub struct NtCatalogCapabilityProofDocument",
    "pub struct AmbientCredentialScrubPlan",
    "pub struct NtCatalogSsmParameterRefs",
    "pub struct NtCatalogCapabilityProof",
    "pub struct NtCatalogCapabilityControls",
    "pub struct NtCatalogCapabilityEvidence",
    "pub struct NtCatalogReadBackEvidence",
    "pub enum NtCatalogCredentialSource",
    "NtCatalogCredentialSource::Ssm",
    "CreateOnlyProbeTranscript",
    "expected_storage_options_keys",
    "proof_artifact_object_name",
    "proof_artifact_uri",
    "proof_artifact_sha256",
    "pub evidence: NtCatalogCapabilityEvidence",
    "pub fn validate(&self, artifact_root: &ResolvedArtifactRoot)",
    "ssm_parameter_refs",
    "ambient_credential_scrub",
    "profile_file_paths_redirected",
    "imds_blocked",
    "proof_plan",
    "completed_proof",
    "completed_proof_from_evidence",
    "persist_completed_proof",
    "persist_completed_proof_from_evidence",
    "from_evidence",
    ".put_create_idempotent(",
    "synthetic_source_proof_id",
    "provenance",
    "no_cloud_feature_gate_failed",
    "ambient_credentials_scrubbed",
    "invalid_credentials_write_failed",
    "ssm_credentials_write_reopen_query_succeeded",
    "query_files_succeeded",
    "query_files_result_count",
    "query_instruments_succeeded",
    "query_instruments_result_count",
    "binary_option_instrument_read_back",
    "binary_option_instrument_id",
    "perps_spot_instrument_read_back",
    "perps_spot_instrument_id",
    "create_only_probe",
    "conditional_put_probe_succeeded",
    "copy_if_not_exists_probe_succeeded",
    "synthetic_fixture_coverage",
    "MarketStructureFixture::BinaryOption",
    "MarketStructureFixture::PerpsSpot",
    "nt_catalog_synthetic_proof_root",
    "direct_s3_catalog_access_proven",
)
OPERATOR_REQUIRED = (
    "ArtifactStoreConfig",
    "CatalogDispatchConfig",
    "pub artifact_store: ArtifactStoreConfig",
    "pub catalog_dispatch: CatalogDispatchConfig",
    "pub create_only_probe_id: String",
    "create_only_probe_transcript",
    "run_from_run_spec_with_artifact_store",
    ".probe_create_only(",
    "persist_catalog_projection_for_source_binding",
)
TEST_REQUIRED = (
    "create_only_probe_requires_duplicate_create_rejection",
    "persists_catalog_projection_directory_with_create_only_dispatch",
    "rejects_duplicate_catalog_projection_bytes",
    "operator_artifact_store_path_persists_catalog_and_rewrites_contract_uri",
    "resolves_synthetic_nt_catalog_proof_root_outside_canonical_catalog",
    "nt_catalog_capability_proof_requires_synthetic_ssm_direct_s3_controls",
    "NtCatalogCapabilityRunSpec",
    "NtCatalogCapabilityProof",
    "NtCatalogCapabilityProofDocument",
    "NtCatalogCredentialSource::Ssm",
    "nt_catalog_capability_proof",
    "proof_plan",
    "completed_proof",
    "completed_proof_from_evidence",
    "persist_completed_proof",
    "persist_completed_proof_from_evidence",
    "NtCatalogCapabilityEvidence",
    "NtCatalogReadBackEvidence",
    "CreateOnlyProbeTranscript",
    "successful_capability_evidence",
    "query_instruments_succeeded = false",
    "query_files_result_count = 0",
    "binary_option_instrument_id",
    ".clear();",
    "duplicate_copy_rejected = false",
    "proof_artifact_uri",
    "proof_artifact_sha256",
    "serde_json::from_slice::<NtCatalogCapabilityProofDocument>",
    "persisted_document.evidence",
    "persisted_document.proof",
    "profile_file_paths_redirected",
    "imds_blocked",
    "direct_s3_catalog_access_proven",
    "run_from_run_spec_with_artifact_store",
    "canonical_catalog_uri",
    "persisted_catalog_objects",
    "create_only_probe_transcript",
    "artifact_uris.nt_catalog_uri",
    "InMemory::new",
    "persist_catalog_projection_for_source_binding",
)
RUN_SPEC_REQUIRED = (
    "create_only_probe_id",
    "[nt_catalog_capability_proof]",
    "proof_run_id",
    "nt_revision",
    "credential_source = \"ssm\"",
    "proof_artifact_object_name",
    "expected_storage_options_keys",
    "synthetic_fixture_coverage",
    "synthetic_source_proof_id = \"synthetic-fixture\"",
    "provenance = \"synthetic\"",
    "[nt_catalog_capability_proof.ambient_credential_scrub]",
    "unset_env_vars",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_DEFAULT_REGION",
    "AWS_REGION",
    "AWS_ENDPOINT",
    "AWS_ENDPOINT_URL_S3",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_ROLE_ARN",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_PROFILE",
    "profile_file_paths_redirected = true",
    "imds_blocked = true",
    "[nt_catalog_capability_proof.ssm_parameter_refs]",
    "access_key_id",
    "secret_access_key",
    "session_token",
    "[artifact_store.s3]",
    "region",
    "conditional_put = \"etag\"",
    "copy_if_not_exists = \"multipart\"",
    "[artifact_store.create_only_probe]",
    "prefix",
    "object_name",
    "copy_source_object_name",
    "copy_dest_object_name",
    "nt_catalog_synthetic_proof",
    "nt-catalog-synthetic-proof",
)


def missing_snippets(path: Path, text: str, snippets: tuple[str, ...]) -> list[str]:
    return [f"{path}: missing `{snippet}`" for snippet in snippets if snippet not in text]


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    cargo_toml = root / BTE_CARGO_TOML
    if not cargo_toml.exists():
        findings.append(f"{BTE_CARGO_TOML}: Cargo.toml is missing")
    else:
        findings.extend(
            missing_snippets(
                BTE_CARGO_TOML,
                cargo_toml.read_text(encoding="utf-8"),
                CARGO_TOML_REQUIRED,
            )
        )

    artifact_store = root / ARTIFACT_STORE
    if not artifact_store.exists():
        findings.append(f"{ARTIFACT_STORE}: artifact_store.rs is missing")
    else:
        artifact_store_text = artifact_store.read_text(encoding="utf-8")
        findings.extend(
            missing_snippets(
                ARTIFACT_STORE,
                artifact_store_text,
                ARTIFACT_STORE_REQUIRED,
            )
        )
        for forbidden in ("AmazonS3Builder::from_env", "parse_url_opts"):
            if forbidden in artifact_store_text:
                findings.append(
                    f"{ARTIFACT_STORE}: forbidden hidden S3 config fallback `{forbidden}`"
                )

    capability_proof = root / CAPABILITY_PROOF
    if not capability_proof.exists():
        findings.append(f"{CAPABILITY_PROOF}: NT catalog capability proof module is missing")
    else:
        findings.extend(
            missing_snippets(
                CAPABILITY_PROOF,
                capability_proof.read_text(encoding="utf-8"),
                CAPABILITY_PROOF_REQUIRED,
            )
        )

    lib = root / LIB_PATH
    if not lib.exists():
        findings.append(f"{LIB_PATH}: lib.rs is missing")
    elif "pub mod nt_catalog_capability;" not in lib.read_text(encoding="utf-8"):
        findings.append(f"{LIB_PATH}: missing public nt_catalog_capability module export")

    operator = root / OPERATOR
    if not operator.exists():
        findings.append(f"{OPERATOR}: operator.rs is missing")
    else:
        text = operator.read_text(encoding="utf-8")
        findings.extend(missing_snippets(OPERATOR, text, OPERATOR_REQUIRED))

    test_file = root / CONTRACT_TEST
    if not test_file.exists():
        findings.append(f"{CONTRACT_TEST}: artifact store contract test is missing")
    else:
        findings.extend(
            missing_snippets(
                CONTRACT_TEST,
                test_file.read_text(encoding="utf-8"),
                TEST_REQUIRED,
            )
        )

    run_spec = root / RUN_SPEC
    if not run_spec.exists():
        findings.append(f"{RUN_SPEC}: committed run spec is missing")
    else:
        findings.extend(
            missing_snippets(RUN_SPEC, run_spec.read_text(encoding="utf-8"), RUN_SPEC_REQUIRED)
        )

    justfile = root / JUSTFILE
    if not justfile.exists():
        findings.append(f"{JUSTFILE}: justfile is missing")
    elif "verify-ra-gate0-catalog-persistence" not in justfile.read_text(encoding="utf-8"):
        findings.append(f"{JUSTFILE}: missing verify-ra-gate0-catalog-persistence recipe")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA Gate-0 catalog persistence violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA Gate-0 catalog persistence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
