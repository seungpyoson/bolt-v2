#!/usr/bin/env python3
"""Self-tests for the RA Gate-0 catalog persistence verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_gate0_catalog_persistence.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_gate0_catalog_persistence", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel: str, text: str) -> Path:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def compliant_artifact_store() -> str:
    return """
use ahash::AHashMap;
use object_store::aws::{AmazonS3, AmazonS3Builder, S3ConditionalPut, S3CopyIfNotExists};

pub struct S3ArtifactStoreConfig;
pub struct CreateOnlyProbeConfig {
    pub copy_source_object_name: String,
    pub copy_dest_object_name: String,
}
pub struct CreateOnlyProbeTranscript {
    pub duplicate_create_rejected: bool,
    pub duplicate_copy_rejected: bool,
}

pub struct ArtifactStoreConfig {
    pub s3: S3ArtifactStoreConfig,
    pub subpaths: ArtifactSubpaths,
}
pub struct ArtifactSubpaths {
    pub nt_catalog_synthetic_proof: String,
}

impl ArtifactStoreConfig {
    pub fn build_s3_object_store(&self) -> Result<AmazonS3> {
        AmazonS3Builder::new()
            .with_bucket_name("configured-bucket")
            .with_region("configured-region")
            .with_conditional_put(S3ConditionalPut::ETagMatch)
            .with_copy_if_not_exists(S3CopyIfNotExists::Multipart)
            .build()
    }

    pub fn nt_catalog_storage_options(&self) -> Result<AHashMap<String, String>> {
        Ok(self.s3.nt_catalog_storage_options())
    }
}

impl S3ArtifactStoreConfig {
    pub fn nt_catalog_storage_options(&self) -> AHashMap<String, String> {
        let mut options = AHashMap::new();
        options.insert("region".to_string(), "configured-region".to_string());
        options
    }
}

fn create_only_probe_uri() {}

fn nt_catalog_synthetic_proof_root() {
    let _root = subpaths.nt_catalog_synthetic_proof;
}

impl CatalogDispatchConfig {
    pub fn catalog_root_for(&self) {}
}

impl CreateOnlyArtifactWriter {
    pub async fn probe_create_only() {
        let _duplicate_create_rejected = true;
        let _duplicate_copy_rejected = true;
        store.copy_if_not_exists(source, dest).await?;
    }
}

pub async fn persist_catalog_projection_for_source_binding() {
    let _dispatch: CatalogDispatchConfig;
    let _root = dispatch.catalog_root_for(source_binding, artifact_root)?;
    pub binding: CatalogProjectionBinding,
    let _persisted = PersistedCatalogProjection {
        binding,
    };
    let writer = CreateOnlyArtifactWriter::new(store);
    let bytes = fs::read(path)?;
    writer.put_create_idempotent(path, bytes).await?;
}
"""


def compliant_capability_proof() -> str:
    return """
use crate::{
    artifact_store::{CreateOnlyArtifactWriter, CreateOnlyProbeTranscript, ResolvedArtifactRoot},
    run_manifest::MarketStructureFixture,
};

pub const NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION: &str = "nt-catalog-capability-proof.v1";
pub const SYNTHETIC_SOURCE_PROOF_ID: &str = "synthetic-fixture";
pub const REQUIRED_AMBIENT_AWS_CREDENTIAL_ENV_VARS: &[&str] = &["AWS_ACCESS_KEY_ID"];

pub enum NtCatalogCredentialSource { Ssm }
pub struct NtCatalogSsmParameterRefs;
pub struct AmbientCredentialScrubPlan {
    pub unset_env_vars: Vec<String>,
    pub profile_file_paths_redirected: bool,
    pub imds_blocked: bool,
}
pub struct NtCatalogCapabilityRunSpec {
    pub credential_source: NtCatalogCredentialSource,
    pub proof_artifact_object_name: String,
    pub expected_storage_options_keys: Vec<String>,
    pub synthetic_source_proof_id: String,
    pub provenance: String,
    pub synthetic_fixture_coverage: Vec<MarketStructureFixture>,
    pub ambient_credential_scrub: AmbientCredentialScrubPlan,
    pub ssm_parameter_refs: NtCatalogSsmParameterRefs,
}
pub struct NtCatalogCapabilityPlan;
pub struct NtCatalogCapabilityProofArtifact {
    pub proof_artifact_uri: String,
    pub proof_artifact_sha256: String,
    pub evidence: NtCatalogCapabilityEvidence,
}
pub struct NtCatalogCapabilityControls {
    pub no_cloud_feature_gate_failed: bool,
    pub ambient_credentials_scrubbed: bool,
    pub invalid_credentials_write_failed: bool,
    pub ssm_credentials_write_reopen_query_succeeded: bool,
    pub conditional_put_probe_succeeded: bool,
    pub copy_if_not_exists_probe_succeeded: bool,
}
pub struct NtCatalogReadBackEvidence {
    pub query_files_succeeded: bool,
    pub query_files_result_count: usize,
    pub query_instruments_succeeded: bool,
    pub query_instruments_result_count: usize,
    pub binary_option_instrument_read_back: bool,
    pub binary_option_instrument_id: String,
    pub perps_spot_instrument_read_back: bool,
    pub perps_spot_instrument_id: String,
}
pub struct NtCatalogCapabilityEvidence {
    pub no_cloud_feature_gate_failed: bool,
    pub ambient_credentials_scrubbed: bool,
    pub invalid_credentials_write_failed: bool,
    pub ssm_credentials_write_reopen_query_succeeded: bool,
    pub nt_catalog_storage_option_keys: Vec<String>,
    pub read_back: NtCatalogReadBackEvidence,
    pub create_only_probe: CreateOnlyProbeTranscript,
}
pub struct NtCatalogCapabilityProof {
    pub synthetic_source_proof_id: String,
    pub provenance: String,
    pub synthetic_fixture_coverage: Vec<MarketStructureFixture>,
}
pub struct NtCatalogCapabilityProofDocument {
    pub proof: NtCatalogCapabilityProof,
    pub evidence: NtCatalogCapabilityEvidence,
}

impl NtCatalogCapabilityProofDocument {
    pub fn validate(&self, artifact_root: &ResolvedArtifactRoot) {
        self.proof.direct_s3_catalog_access_proven(artifact_root);
        let _evidence = &self.evidence;
    }
}

impl NtCatalogCapabilityProof {
    pub fn from_evidence(evidence: &NtCatalogCapabilityEvidence) -> NtCatalogCapabilityControls {
        let _query_files = evidence.read_back.query_files_succeeded;
        let _query_instruments = evidence.read_back.query_instruments_succeeded;
        let _binary_option = evidence.read_back.binary_option_instrument_read_back;
        let _perps_spot = evidence.read_back.perps_spot_instrument_read_back;
        let _probe = &evidence.create_only_probe;
        NtCatalogCapabilityControls {
            no_cloud_feature_gate_failed: evidence.no_cloud_feature_gate_failed,
            ambient_credentials_scrubbed: evidence.ambient_credentials_scrubbed,
            invalid_credentials_write_failed: evidence.invalid_credentials_write_failed,
            ssm_credentials_write_reopen_query_succeeded: evidence.ssm_credentials_write_reopen_query_succeeded,
            conditional_put_probe_succeeded: true,
            copy_if_not_exists_probe_succeeded: true,
        }
    }
    pub fn proof_plan(&self) -> NtCatalogCapabilityPlan { NtCatalogCapabilityPlan }
    pub fn completed_proof(&self) -> Self { Self {
        synthetic_source_proof_id: self.synthetic_source_proof_id.clone(),
        provenance: self.provenance.clone(),
        synthetic_fixture_coverage: self.synthetic_fixture_coverage.clone(),
    } }
    pub fn completed_proof_from_evidence(&self, evidence: &NtCatalogCapabilityEvidence) -> Self {
        let _controls = Self::from_evidence(evidence);
        self.completed_proof()
    }
    pub async fn persist_completed_proof(&self, writer: &CreateOnlyArtifactWriter<'_>) -> NtCatalogCapabilityProofArtifact {
        writer.put_create_idempotent(path, bytes).await.unwrap();
        NtCatalogCapabilityProofArtifact {
            proof_artifact_uri: "s3://bucket/nt-catalog-synthetic-proof/v1/proof=proof-run/nt-catalog-capability-proof.json".to_string(),
            proof_artifact_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            evidence: NtCatalogCapabilityEvidence {},
        }
    }
    pub async fn persist_completed_proof_from_evidence(&self, writer: &CreateOnlyArtifactWriter<'_>, evidence: &NtCatalogCapabilityEvidence) -> NtCatalogCapabilityProofArtifact {
        let _proof = self.completed_proof_from_evidence(evidence);
        self.persist_completed_proof(writer).await
    }

    pub fn direct_s3_catalog_access_proven(&self, artifact_root: &ResolvedArtifactRoot) {
        let _root = artifact_root.nt_catalog_synthetic_proof_root("proof-run").unwrap();
        let _credential_source = NtCatalogCredentialSource::Ssm;
        let _fixtures = [MarketStructureFixture::BinaryOption, MarketStructureFixture::PerpsSpot];
    }
}
"""


def compliant_operator() -> str:
    return f"""
use crate::artifact_store::{{ArtifactStoreConfig, CatalogDispatchConfig}};

pub struct RunSpec {{
    pub artifact_store: ArtifactStoreConfig,
    pub catalog_dispatch: CatalogDispatchConfig,
    pub create_only_probe_id: String,
}}

pub fn run_from_run_spec_with_artifact_store() {{
    let create_only_probe_transcript = writer.probe_create_only();
    persist_catalog_projection_for_source_binding();
}}
"""


def compliant_test() -> str:
    return """
fn create_only_probe_requires_duplicate_create_rejection() {
    let _store = InMemory::new();
}

fn persists_catalog_projection_directory_with_create_only_dispatch() {
    let _store = InMemory::new();
    persist_catalog_projection_for_source_binding();
}

fn rejects_duplicate_catalog_projection_bytes() {}

fn operator_artifact_store_path_persists_catalog_and_rewrites_contract_uri() {
    let _store = InMemory::new();
    let artifacts = run_from_run_spec_with_artifact_store();
    let _canonical_catalog_uri = artifacts.canonical_catalog_uri;
    let _persisted_catalog_objects = artifacts.persisted_catalog_objects;
    let _persisted_source_binding = persisted.binding.source_binding;
    let _persisted_market_structure = persisted.binding.market_structure_fixture;
    let _persisted_projection_id = persisted.binding.catalog_projection_id;
    let _create_only_probe_transcript = artifacts.create_only_probe_transcript;
    let _nt_catalog_uri = artifacts.output.contract.artifact_uris.nt_catalog_uri;
}

fn resolves_synthetic_nt_catalog_proof_root_outside_canonical_catalog() {}

fn successful_capability_evidence() -> NtCatalogCapabilityEvidence {
    NtCatalogCapabilityEvidence {
        no_cloud_feature_gate_failed: true,
        ambient_credentials_scrubbed: true,
        invalid_credentials_write_failed: true,
        ssm_credentials_write_reopen_query_succeeded: true,
        nt_catalog_storage_option_keys: vec!["region".to_string()],
        read_back: NtCatalogReadBackEvidence {
            query_files_succeeded: true,
            query_files_result_count: 1,
            query_instruments_succeeded: true,
            query_instruments_result_count: 2,
            binary_option_instrument_read_back: true,
            binary_option_instrument_id: String::from("binary-option-synthetic"),
            perps_spot_instrument_read_back: true,
            perps_spot_instrument_id: String::from("perps-spot-synthetic"),
        },
        create_only_probe: CreateOnlyProbeTranscript {
            first_create_succeeded: true,
            duplicate_create_rejected: true,
            first_copy_succeeded: true,
            duplicate_copy_rejected: true,
        },
    }
}

fn nt_catalog_capability_proof_requires_synthetic_ssm_direct_s3_controls() {
    let run_spec: NtCatalogCapabilityRunSpec = nt_catalog_capability_proof;
    let mut evidence = successful_capability_evidence();
    let plan = run_spec.proof_plan();
    let _profile = plan.profile_file_paths_redirected;
    let _imds = plan.imds_blocked;
    let _completed = plan.completed_proof_from_evidence(&evidence);
    let mut mismatched_storage_options_evidence = evidence.clone();
    mismatched_storage_options_evidence.nt_catalog_storage_option_keys = vec!["endpoint_url".to_string()];
    evidence.read_back.query_instruments_succeeded = false;
    evidence.read_back.query_files_result_count = 0;
    evidence.read_back.binary_option_instrument_id
        .clear();
    evidence.create_only_probe.duplicate_copy_rejected = false;
    let proof = NtCatalogCapabilityProof {};
    let persisted = proof.persist_completed_proof_from_evidence(&writer, &evidence);
    let _proof_uri = persisted.proof_artifact_uri;
    let _proof_sha256 = persisted.proof_artifact_sha256;
    let persisted_document = serde_json::from_slice::<NtCatalogCapabilityProofDocument>(&persisted_bytes).unwrap();
    let _document_evidence = persisted_document.evidence;
    let _document_proof = persisted_document.proof;
    let _credential_source = NtCatalogCredentialSource::Ssm;
    proof.direct_s3_catalog_access_proven();
}
"""


def write_compliant_tree(root: Path) -> None:
    write_file(
        root,
        "crates/backtesting-vertical-slice/Cargo.toml",
        'object_store = { version = "=0.13.2", default-features = false, features = ["aws"] }\n',
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/artifact_store.rs",
        compliant_artifact_store(),
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/nt_catalog_capability.rs",
        compliant_capability_proof(),
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/lib.rs",
        "pub mod nt_catalog_capability;\n",
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/operator.rs",
        compliant_operator(),
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/tests/artifact_store_contract.rs",
        compliant_test(),
    )
    write_file(
        root,
        "justfile",
        "verify-ra-gate0-catalog-persistence:\n    python3 scripts/verify_ra_gate0_catalog_persistence.py\n",
    )
    write_file(
        root,
        (
            "specs/023-nt-research-analytics-platform/reference/"
            "backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
        ),
        """
create_only_probe_id = "probe-run"

[nt_catalog_capability_proof]
proof_run_id = "synthetic-capability-proof"
nt_revision = "6e059dcbb59ac1e582132fc431a581936c216c3c"
credential_source = "ssm"
proof_artifact_object_name = "nt-catalog-capability-proof.json"
expected_storage_options_keys = ["region"]
synthetic_fixture_coverage = ["binary-option", "perps-spot"]
synthetic_source_proof_id = "synthetic-fixture"
provenance = "synthetic"

[nt_catalog_capability_proof.ambient_credential_scrub]
unset_env_vars = [
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
]
profile_file_paths_redirected = true
imds_blocked = true

[nt_catalog_capability_proof.ssm_parameter_refs]
access_key_id = "/bolt-v2/research/catalog/aws-access-key-id"
secret_access_key = "/bolt-v2/research/catalog/aws-secret-access-key"
session_token = "/bolt-v2/research/catalog/aws-session-token"

[artifact_store.s3]
region = "us-east-1"
conditional_put = "etag"
copy_if_not_exists = "multipart"

[artifact_store.create_only_probe]
prefix = ".writer-probe"
object_name = "sentinel"
copy_source_object_name = "copy-source"
copy_dest_object_name = "copy-dest"

[artifact_store.subpaths]
nt_catalog_synthetic_proof = "nt-catalog-synthetic-proof"
""",
    )


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_compliant_tree_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)

        assert verifier.scan_root(root) == []


def test_missing_persistence_helper_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)
        write_file(root, "crates/backtesting-vertical-slice/src/artifact_store.rs", "")

        findings = verifier.scan_root(root)

    assert any("persist_catalog_projection_for_source_binding" in finding for finding in findings)


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)
        write_file(root, "crates/backtesting-vertical-slice/src/operator.rs", "")

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "ArtifactStoreConfig" in result.stderr


def main() -> int:
    tests = [
        test_compliant_tree_passes,
        test_missing_persistence_helper_is_a_finding,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: RA Gate-0 catalog persistence verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
