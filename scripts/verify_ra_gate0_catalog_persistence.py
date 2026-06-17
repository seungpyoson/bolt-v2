#!/usr/bin/env python3
"""Verify RA Gate-0 catalog persistence is wired into the operator path."""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ARTIFACT_STORE = Path("crates/backtesting-vertical-slice/src/artifact_store.rs")
CAPABILITY_PROOF = Path("crates/backtesting-vertical-slice/src/nt_catalog_capability.rs")
LIB_PATH = Path("crates/backtesting-vertical-slice/src/lib.rs")
OPERATOR = Path("crates/backtesting-vertical-slice/src/operator.rs")
MAIN_RS = Path("crates/backtesting-vertical-slice/src/main.rs")
CONTRACT_TEST = Path("crates/backtesting-vertical-slice/tests/artifact_store_contract.rs")
BTE_CARGO_TOML = Path("crates/backtesting-vertical-slice/Cargo.toml")
JUSTFILE = Path("justfile")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")
RUN_SPEC = Path(
    "specs/023-nt-research-analytics-platform/reference/"
    "backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
)
CHECKED_RA001 = re.compile(r"^- \[[xX]\] RA-001\b", re.MULTILINE)

CARGO_TOML_REQUIRED = (
    'aws-config = "=1.8.18"',
    'aws-sdk-ssm = { version = "=1.113.0", default-features = false, features = ["default-https-client", "rt-tokio"] }',
    'object_store = { version = "=0.13.2", default-features = false, features = ["aws"] }',
)
ARTIFACT_STORE_REQUIRED = (
    "AHashMap<String, String>",
    "object_store::aws::{AmazonS3, AmazonS3Builder, S3ConditionalPut, S3CopyIfNotExists}",
    "pub struct S3ArtifactStoreConfig",
    "pub struct S3ArtifactStoreCredentials",
    "pub s3: S3ArtifactStoreConfig",
    "pub catalog_projection_manifest_object: String",
    "pub fn build_s3_object_store_with_credentials",
    "pub fn nt_catalog_storage_options(&self) -> Result<AHashMap<String, String>>",
    "pub fn nt_catalog_storage_options(&self) -> AHashMap<String, String>",
    "pub fn nt_catalog_storage_options_with_credentials",
    ".insert(\"region\".to_string(),",
    "\"access_key_id\".to_string()",
    "credentials.access_key_id().to_string()",
    "\"secret_access_key\".to_string()",
    "credentials.secret_access_key().to_string()",
    ".insert(\"session_token\".to_string(),",
    "credentials.session_token()",
    ".with_bucket_name(",
    ".with_region(",
    ".with_access_key_id(",
    ".with_secret_access_key(",
    ".with_token(",
    ".with_conditional_put(match self.s3.conditional_put",
    "S3ConditionalPutMode::Etag => S3ConditionalPut::ETagMatch",
    ".with_copy_if_not_exists(match self.s3.copy_if_not_exists",
    "S3CopyIfNotExistsMode::Multipart => S3CopyIfNotExists::Multipart",
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
    "CreateOnlyWriteDisposition",
    "pub binding: CatalogProjectionBinding",
    "pub manifest_uri: String",
    "pub relative_path: String",
    "pub manifest_sha256: String",
    "pub manifest_create_only_write: CreateOnlyWriteDisposition",
    "pub create_only_write: CreateOnlyWriteDisposition",
    "catalog_projection_manifest_object_uri",
    "CatalogProjectionManifestDocument",
    "CatalogProjectionManifestObject",
    "catalog-projection-manifest-v1",
    "expected_market_structure_fixture",
    "binding.market_structure_fixture == expected_market_structure_fixture",
    "catalog_projection_manifest_sha256",
    "put_create_idempotent_with_disposition",
    "CreateOnlyWriteDisposition::Created",
    "CreateOnlyWriteDisposition::AlreadyExistedSamePayload",
    "pub fn catalog_root_for(",
    "binding,",
    "CreateOnlyArtifactWriter::new",
    ".put_create_idempotent(",
    "fs::read(",
)
CAPABILITY_PROOF_REQUIRED = (
    "use aws_config::BehaviorVersion",
    "use aws_sdk_ssm::{Client as SsmClient, config::Region}",
    "S3ArtifactStoreCredentials",
    "pub const NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION",
    "pub const SYNTHETIC_SOURCE_PROOF_ID",
    "pub const REQUIRED_AMBIENT_AWS_CREDENTIAL_ENV_VARS",
    "pub struct NtCatalogSsmCredentialResolver",
    "pub async fn from_region",
    "pub async fn resolve(",
    "refs: &NtCatalogSsmParameterRefs",
    "async fn resolve_required_parameter",
    ".with_decryption(true)",
    "pub struct NtCatalogCapabilityRunSpec",
    "pub struct NtCatalogCapabilityPlan",
    "pub struct NtCatalogCapabilityProofArtifact",
    "pub struct NtCatalogCapabilityProofDocument",
    "pub struct NtCatalogS3ConformanceProbe",
    "pub struct AmbientCredentialScrubPlan",
    "pub struct NtCatalogSsmParameterRefs",
    "pub struct NtCatalogCapabilityProof",
    "pub struct NtCatalogCapabilityControls",
    "pub struct NtCatalogCapabilityEvidence",
    "pub struct NtCatalogReadBackEvidence",
    "pub catalog_uri: String",
    "pub enum NtCatalogCredentialSource",
    "NtCatalogCredentialSource::Ssm",
    "CreateOnlyProbeTranscript",
    "expected_storage_options_keys",
    "proof_artifact_object_name",
    "proof_artifact_uri",
    "proof_artifact_sha256",
    "proof_artifact_create_only_write",
    "CreateOnlyWriteDisposition",
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
    "put_create_idempotent_with_disposition",
    "synthetic_source_proof_id",
    "provenance",
    "no_cloud_feature_gate_failed",
    "ambient_credentials_scrubbed",
    "invalid_credentials_write_failed",
    "ssm_credentials_write_reopen_query_succeeded",
    "nt_catalog_storage_option_keys",
    "query_files_succeeded",
    "query_files_result_count",
    "write_instruments_succeeded",
    "write_trade_ticks_succeeded",
    "query_trade_ticks_succeeded",
    "query_trade_ticks_result_count",
    "read_back.catalog_uri",
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
    "synthetic_fixtures",
    "runtime_evidence",
    "runtime_is_scrubbed",
    "invalid_credentials_write_fails",
    "s3_conformance_probe",
    "MarketStructureFixture::BinaryOption",
    "MarketStructureFixture::PerpsSpot",
    "nt_catalog_synthetic_proof_root",
    "run_nt_catalog_s3_conformance_probe",
    "ParquetDataCatalog::from_uri",
    ".write_instruments(",
    ".write_to_parquet(",
    ".query_files(",
    ".query_instruments(",
    ".query_typed_data::<TradeTick>(",
    "direct_s3_catalog_access_proven",
)
OPERATOR_REQUIRED = (
    "ArtifactStoreConfig",
    "CatalogDispatchConfig",
    "NtCatalogCapabilityPlan",
    "NtCatalogCapabilityRunSpec",
    "pub artifact_store: Option<ArtifactStoreConfig>",
    "pub catalog_dispatch: Option<CatalogDispatchConfig>",
    "pub create_only_probe_id: Option<String>",
    "pub nt_catalog_capability_proof: Option<NtCatalogCapabilityRunSpec>",
    "pub fn required_artifact_store(&self) -> Result<&ArtifactStoreConfig>",
    "pub fn required_catalog_dispatch(&self) -> Result<&CatalogDispatchConfig>",
    "pub fn required_create_only_probe_id(&self) -> Result<&str>",
    "pub fn required_nt_catalog_capability_proof(&self) -> Result<&NtCatalogCapabilityRunSpec>",
    "pub nt_catalog_capability_plan: Option<NtCatalogCapabilityPlan>",
    "pub nt_catalog_capability_proof_artifact: Option<NtCatalogCapabilityProofArtifact>",
    "create_only_probe_transcript",
    "run_from_run_spec_with_artifact_store",
    "build_capability_evidence",
    "let artifact_store = spec.required_artifact_store()?",
    "let catalog_dispatch = spec.required_catalog_dispatch()?",
    "let create_only_probe_id = spec.required_create_only_probe_id()?",
    "let nt_catalog_capability_proof = spec.required_nt_catalog_capability_proof()?",
    "nt_catalog_capability_proof.proof_plan(artifact_store)?",
    ".probe_create_only(",
    ".persist_completed_proof_from_evidence(",
    "fs::remove_dir_all(&artifacts.catalog_root)",
    "persist_catalog_projection_for_source_binding",
)
MAIN_REQUIRED = (
    "run_from_run_spec_with_artifact_store",
    "NtCatalogSsmCredentialResolver",
    "NtCatalogSsmCredentialResolver::from_region",
    "let artifact_store = spec.required_artifact_store()?",
    "let nt_catalog_capability_proof = spec.required_nt_catalog_capability_proof()?",
    ".resolve(&nt_catalog_capability_proof.ssm_parameter_refs)",
    ".build_s3_object_store_with_credentials(&credentials)",
    ".runtime_evidence(",
    "#[tokio::main",
)
MAIN_FORBIDDEN = (
    "local_nt_catalog_root",
)
TEST_REQUIRED = (
    "S3ArtifactStoreCredentials",
    "s3_credentials_reject_blank_resolved_values",
    ".build_s3_object_store_with_credentials(&credentials)",
    "create_only_probe_requires_duplicate_create_rejection",
    "persists_catalog_projection_directory_with_create_only_dispatch",
    "rejects_duplicate_catalog_projection_bytes",
    "rejects_catalog_dispatch_fixture_mismatch",
    "rejects_manifest_fixture_mismatch",
    "FixtureMismatch",
    "accepted.fixture_type",
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
    "nt_catalog_storage_option_keys: vec![\"region\".to_string()]",
    "mismatched_storage_options_evidence",
    "mismatched_read_back_catalog_uri_evidence",
    "nt_catalog_storage_option_keys",
    "vec![\"endpoint_url\".to_string()]",
    "nt_catalog_projection_root(\"canonical-projection\")",
    "query_instruments_succeeded = false",
    "query_files_result_count = 0",
    "query_trade_ticks_result_count = 0",
    "binary_option_instrument_id",
    ".clear();",
    "duplicate_copy_rejected = false",
    "proof_artifact_uri",
    "proof_artifact_sha256",
    "proof_artifact_create_only_write",
    "same proof artifact bytes are idempotent",
    "changed_valid_evidence",
    "query_files_result_count += 1",
    "serde_json::from_slice::<NtCatalogCapabilityProofDocument>",
    "persisted_document.evidence",
    "persisted_document.proof",
    "profile_file_paths_redirected",
    "imds_blocked",
    "direct_s3_catalog_access_proven",
    "run_from_run_spec_with_artifact_store",
    "canonical_catalog_uri",
    "nt_catalog_capability_plan",
    ".nt_catalog_capability_plan",
    ".nt_catalog_capability_proof_artifact",
    "proof_artifact.evidence",
    "transient local NT catalog",
    "NT catalog capability proof plan",
    "persisted_catalog_objects",
    "persisted_catalog_projection",
    "persisted_projection.manifest_create_only_write",
    "persisted.binding.source_binding",
    "persisted.binding.market_structure_fixture",
    "MarketStructureFixture::PerpsSpot",
    "market_structure_fixture mismatch",
    "persisted.binding.catalog_projection_id",
    "expected_catalog_projection_manifest_sha256",
    "persisted.manifest_sha256",
    "relative_path",
    "manifest_uri",
    "manifest_create_only_write",
    "CreateOnlyWriteDisposition::Created",
    "CreateOnlyWriteDisposition::AlreadyExistedSamePayload",
    "create_only_write",
    "create_only_probe_transcript",
    "artifacts.output.contract.catalog_hash",
    "artifact_uris.nt_catalog_uri",
    "nt_catalog_manifest_uri",
    "InMemory::new",
    "persist_catalog_projection_for_source_binding",
)
RUN_SPEC_REQUIRED = (
    "create_only_probe_id",
    "catalog_projection_manifest_object",
    "[nt_catalog_capability_proof]",
    "proof_run_id",
    "nt_revision",
    "credential_source = \"ssm\"",
    "proof_artifact_object_name",
    "expected_storage_options_keys",
    "synthetic_fixture_coverage",
    "[nt_catalog_capability_proof.synthetic_fixtures.binary_option]",
    "[nt_catalog_capability_proof.synthetic_fixtures.perps_spot]",
    "[[nt_catalog_capability_proof.synthetic_fixtures.trade_ticks]]",
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

RUST_LITERAL_REQUIRED_SNIPPETS = {
    "FixtureMismatch",
    "accepted.fixture_type",
}


def missing_snippets(path: Path, text: str, snippets: tuple[str, ...]) -> list[str]:
    return [f"{path}: missing `{snippet}`" for snippet in snippets if snippet not in text]


def strip_rust_comments(text: str) -> str:
    out: list[str] = []
    i = 0
    state = "code"
    block_depth = 0
    while i < len(text):
        raw_end = raw_string_end(text, i)
        if state == "code" and raw_end is not None:
            out.append(text[i:raw_end])
            i = raw_end
            continue
        c = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""
        if state == "code":
            if c == "/" and nxt == "/":
                state = "line_comment"
                out.extend("  ")
                i += 2
                continue
            if c == "/" and nxt == "*":
                state = "block_comment"
                block_depth = 1
                out.extend("  ")
                i += 2
                continue
            if c == '"':
                state = "string"
                out.append(c)
                i += 1
                continue
            out.append(c)
            i += 1
            continue
        if state == "line_comment":
            if c == "\n":
                state = "code"
                out.append(c)
            else:
                out.append(" ")
            i += 1
            continue
        if state == "block_comment":
            if c == "/" and nxt == "*":
                block_depth += 1
                out.extend("  ")
                i += 2
                continue
            if c == "*" and nxt == "/":
                block_depth -= 1
                out.extend("  ")
                i += 2
                if block_depth == 0:
                    state = "code"
                continue
            out.append("\n" if c == "\n" else " ")
            i += 1
            continue
        if state == "string":
            out.append(c)
            if c == "\\":
                if i + 1 < len(text):
                    out.append(text[i + 1])
                i += 2
                continue
            if c == '"':
                state = "code"
            i += 1
            continue
    return "".join(out)


def raw_string_end(text: str, i: int) -> int | None:
    start = i
    if text.startswith("br", i):
        i += 2
    elif text.startswith("r", i):
        i += 1
    else:
        return None
    hashes = 0
    while i < len(text) and text[i] == "#":
        hashes += 1
        i += 1
    if i >= len(text) or text[i] != '"':
        return None
    closing = '"' + ("#" * hashes)
    end = text.find(closing, i + 1)
    if end == -1:
        return len(text)
    return end + len(closing)


def strip_rust_comments_and_literals(text: str) -> str:
    out: list[str] = []
    i = 0
    state = "code"
    block_depth = 0
    while i < len(text):
        raw_end = raw_string_end(text, i)
        if state == "code" and raw_end is not None:
            out.append('""')
            out.extend("\n" for _ in range(text.count("\n", i, raw_end)))
            i = raw_end
            continue
        c = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""
        if state == "code":
            if c == "/" and nxt == "/":
                state = "line_comment"
                out.extend("  ")
                i += 2
                continue
            if c == "/" and nxt == "*":
                state = "block_comment"
                block_depth = 1
                out.extend("  ")
                i += 2
                continue
            if c == '"':
                state = "string"
                out.extend('""')
                i += 1
                continue
            out.append(c)
            i += 1
            continue
        if state == "line_comment":
            if c == "\n":
                state = "code"
                out.append(c)
            else:
                out.append(" ")
            i += 1
            continue
        if state == "block_comment":
            if c == "/" and nxt == "*":
                block_depth += 1
                out.extend("  ")
                i += 2
                continue
            if c == "*" and nxt == "/":
                block_depth -= 1
                out.extend("  ")
                i += 2
                if block_depth == 0:
                    state = "code"
                continue
            out.append("\n" if c == "\n" else " ")
            i += 1
            continue
        if state == "string":
            if c == "\\":
                i += 2
                continue
            if c == '"':
                state = "code"
            i += 1
            continue
    return "".join(out)


def snippet_requires_literal_preservation(snippet: str) -> bool:
    if snippet in RUST_LITERAL_REQUIRED_SNIPPETS:
        return True
    if '"' in snippet or "-" in snippet:
        return True
    if " " not in snippet:
        return False
    code_markers = (
        "pub ",
        "fn ",
        "let ",
        "struct ",
        "enum ",
        "impl ",
        "=",
        "?",
        "(",
        ")",
        ".",
        "::",
        "[",
        "]",
        "{",
        "}",
        ",",
        ";",
    )
    return not any(marker in snippet for marker in code_markers)


def missing_rust_snippets(path: Path, text: str, snippets: tuple[str, ...]) -> list[str]:
    comments_stripped = strip_rust_comments(text)
    comments_and_literals_stripped = strip_rust_comments_and_literals(text)
    findings = []
    for snippet in snippets:
        search_text = (
            comments_stripped
            if snippet_requires_literal_preservation(snippet)
            else comments_and_literals_stripped
        )
        if snippet not in search_text:
            findings.append(f"{path}: missing `{snippet}`")
    return findings


def validate_run_spec_toml(path: Path, text: str) -> list[str]:
    try:
        parsed = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        return [f"{path}: invalid TOML: {exc}"]

    findings = []
    artifact_store = parsed.get("artifact_store")
    if not isinstance(artifact_store, dict):
        findings.append(f"{path}: missing [artifact_store] table")
        return findings
    if "catalog_projection_manifest_object" in parsed:
        findings.append(
            f"{path}: catalog_projection_manifest_object must live under [artifact_store]"
        )
    if artifact_store.get("catalog_projection_manifest_object") != (
        "catalog-projection-manifest.json"
    ):
        findings.append(
            f"{path}: missing [artifact_store].catalog_projection_manifest_object"
        )
    return findings


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    tasks = root / TASKS_PATH
    if not tasks.exists():
        findings.append(f"{TASKS_PATH}: tasks.md is missing")
    elif not CHECKED_RA001.search(tasks.read_text(encoding="utf-8")):
        findings.append(f"{TASKS_PATH}: RA-001 must be checked once Gate-0 catalog persistence is implemented")

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
            missing_rust_snippets(
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
            missing_rust_snippets(
                CAPABILITY_PROOF,
                capability_proof.read_text(encoding="utf-8"),
                CAPABILITY_PROOF_REQUIRED,
            )
        )

    lib = root / LIB_PATH
    if not lib.exists():
        findings.append(f"{LIB_PATH}: lib.rs is missing")
    elif missing_rust_snippets(
        LIB_PATH,
        lib.read_text(encoding="utf-8"),
        ("pub mod nt_catalog_capability;",),
    ):
        findings.append(f"{LIB_PATH}: missing public nt_catalog_capability module export")

    operator = root / OPERATOR
    if not operator.exists():
        findings.append(f"{OPERATOR}: operator.rs is missing")
    else:
        text = operator.read_text(encoding="utf-8")
        findings.extend(missing_rust_snippets(OPERATOR, text, OPERATOR_REQUIRED))
        operator_code = strip_rust_comments_and_literals(text)
        if re.search(r"\bfs\s*::\s*write\s*\(\s*&\s*artifacts\s*\.\s*contract_path\b", operator_code):
            findings.append(
                f"{OPERATOR}: durable result contract rewrite must use atomic_write"
            )

    main_rs = root / MAIN_RS
    if not main_rs.exists():
        findings.append(f"{MAIN_RS}: main.rs is missing")
    else:
        main_text = main_rs.read_text(encoding="utf-8")
        findings.extend(missing_rust_snippets(MAIN_RS, main_text, MAIN_REQUIRED))
        if ".build_s3_object_store()" in main_text:
            findings.append(
                f"{MAIN_RS}: runtime S3 object store must use SSM-resolved explicit credentials"
            )
        for forbidden in MAIN_FORBIDDEN:
            if forbidden in main_text:
                findings.append(
                    f"{MAIN_RS}: must not expose transient local catalog output `{forbidden}`"
                )

    test_file = root / CONTRACT_TEST
    if not test_file.exists():
        findings.append(f"{CONTRACT_TEST}: artifact store contract test is missing")
    else:
        findings.extend(
            missing_rust_snippets(
                CONTRACT_TEST,
                test_file.read_text(encoding="utf-8"),
                TEST_REQUIRED,
            )
        )

    run_spec = root / RUN_SPEC
    if not run_spec.exists():
        findings.append(f"{RUN_SPEC}: committed run spec is missing")
    else:
        run_spec_text = run_spec.read_text(encoding="utf-8")
        findings.extend(missing_snippets(RUN_SPEC, run_spec_text, RUN_SPEC_REQUIRED))
        findings.extend(validate_run_spec_toml(RUN_SPEC, run_spec_text))

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
