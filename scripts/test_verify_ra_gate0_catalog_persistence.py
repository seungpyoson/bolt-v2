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
pub struct S3ArtifactStoreCredentials;
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
    pub catalog_projection_manifest_object: String,
    pub subpaths: ArtifactSubpaths,
}
pub struct ArtifactSubpaths {
    pub nt_catalog_synthetic_proof: String,
}

impl ArtifactStoreConfig {
    pub fn build_s3_object_store_with_credentials(
        &self,
        credentials: &S3ArtifactStoreCredentials,
    ) -> Result<AmazonS3> {
        AmazonS3Builder::new()
            .with_bucket_name("configured-bucket")
            .with_region("configured-region")
            .with_access_key_id(credentials.access_key_id())
            .with_secret_access_key(credentials.secret_access_key())
            .with_token(credentials.session_token().unwrap())
            .with_conditional_put(match self.s3.conditional_put {
                S3ConditionalPutMode::Etag => S3ConditionalPut::ETagMatch,
            })
            .with_copy_if_not_exists(match self.s3.copy_if_not_exists {
                S3CopyIfNotExistsMode::Multipart => S3CopyIfNotExists::Multipart,
            })
            .build()
    }

    pub fn nt_catalog_storage_options(&self) -> Result<AHashMap<String, String>> {
        Ok(self.s3.nt_catalog_storage_options())
    }

    pub fn nt_catalog_storage_options_with_credentials(
        &self,
        credentials: &S3ArtifactStoreCredentials,
    ) -> Result<AHashMap<String, String>> {
        let mut options = self.s3.nt_catalog_storage_options();
        options.insert("access_key_id".to_string(), credentials.access_key_id().to_string());
        options.insert("secret_access_key".to_string(), credentials.secret_access_key().to_string());
        if let Some(token) = credentials.session_token() {
            options.insert("session_token".to_string(), token.to_string());
        }
        Ok(options)
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
    pub fn catalog_root_for(
        &self,
        expected_market_structure_fixture: MarketStructureFixture,
    ) {
        let binding = CatalogProjectionBinding {
            source_binding: String::new(),
            market_structure_fixture: expected_market_structure_fixture,
            catalog_projection_id: String::new(),
        };
        assert!(binding.market_structure_fixture == expected_market_structure_fixture);
    }
}

pub enum CreateOnlyWriteDisposition {
    Created,
    AlreadyExistedSamePayload,
}

pub struct PersistedCatalogProjectionObject {
    pub relative_path: String,
    pub create_only_write: CreateOnlyWriteDisposition,
}

pub struct CatalogProjectionManifestDocument;
pub struct CatalogProjectionManifestObject;

pub struct PersistedCatalogProjection {
    pub manifest_uri: String,
    pub manifest_sha256: String,
    pub manifest_create_only_write: CreateOnlyWriteDisposition,
    pub binding: CatalogProjectionBinding,
    pub objects: Vec<PersistedCatalogProjectionObject>,
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
    let expected_market_structure_fixture = MarketStructureFixture::BinaryOption;
    let _root = dispatch.catalog_root_for(source_binding, expected_market_structure_fixture, artifact_root)?;
    pub binding: CatalogProjectionBinding,
    assert!(binding.market_structure_fixture == expected_market_structure_fixture);
    let _manifest_uri = artifact_root.catalog_projection_manifest_object_uri("projection-run-123");
    let _persisted = PersistedCatalogProjection {
        manifest_uri: _manifest_uri,
        manifest_sha256: catalog_projection_manifest_sha256(&objects),
        manifest_create_only_write: CreateOnlyWriteDisposition::Created,
        binding,
        objects: vec![PersistedCatalogProjectionObject {
            relative_path: String::from("data/trade_tick/part-000.parquet"),
            create_only_write: CreateOnlyWriteDisposition::Created,
        }],
    };
    let _schema = "catalog-projection-manifest-v1";
    let writer = CreateOnlyArtifactWriter::new(store);
    let bytes = fs::read(path)?;
    writer.put_create_idempotent(path, bytes.clone()).await?;
    writer.put_create_idempotent_with_disposition(path, bytes).await?;
    let _already_exists = CreateOnlyWriteDisposition::AlreadyExistedSamePayload;
}
"""


def compliant_capability_proof() -> str:
    return """
use aws_config::BehaviorVersion;
use aws_sdk_ssm::{Client as SsmClient, config::Region};

use crate::{
    artifact_store::{CreateOnlyArtifactWriter, CreateOnlyProbeTranscript, ResolvedArtifactRoot, S3ArtifactStoreCredentials},
    run_manifest::MarketStructureFixture,
};

pub const NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION: &str = "nt-catalog-capability-proof.v1";
pub const SYNTHETIC_SOURCE_PROOF_ID: &str = "synthetic-fixture";
pub const REQUIRED_AMBIENT_AWS_CREDENTIAL_ENV_VARS: &[&str] = &["AWS_ACCESS_KEY_ID"];

pub enum NtCatalogCredentialSource { Ssm }
pub struct NtCatalogSsmParameterRefs;
pub struct NtCatalogSsmCredentialResolver {
    client: SsmClient,
}

impl NtCatalogSsmCredentialResolver {
    pub async fn from_region(region: &str) -> Self {
        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region.to_string()))
            .load()
            .await;
        Self { client: SsmClient::new(&config) }
    }

    pub async fn resolve(&self, refs: &NtCatalogSsmParameterRefs) -> S3ArtifactStoreCredentials {
        let _access_key_id = self
            .resolve_required_parameter("ssm_parameter_refs.access_key_id", "path")
            .await;
        let _secret_access_key = self
            .resolve_required_parameter("ssm_parameter_refs.secret_access_key", "path")
            .await;
        let _session_token = self
            .resolve_required_parameter("ssm_parameter_refs.session_token", "path")
            .await;
        let _refs = refs;
        S3ArtifactStoreCredentials
    }

    async fn resolve_required_parameter(&self, label: &str, path: &str) -> String {
        self.client
            .get_parameter()
            .name(path)
            .with_decryption(true)
            .send()
            .await
            .unwrap();
        label.to_string()
    }
}
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
    pub synthetic_fixtures: NtCatalogSyntheticFixtures,
    pub ambient_credential_scrub: AmbientCredentialScrubPlan,
    pub ssm_parameter_refs: NtCatalogSsmParameterRefs,
}
pub struct NtCatalogSyntheticFixtures;
pub struct NtCatalogCapabilityPlan;
pub struct NtCatalogCapabilityProofArtifact {
    pub proof_artifact_uri: String,
    pub proof_artifact_sha256: String,
    pub proof_artifact_create_only_write: CreateOnlyWriteDisposition,
    pub evidence: NtCatalogCapabilityEvidence,
}
pub struct NtCatalogS3ConformanceProbe;
pub struct NtCatalogCapabilityControls {
    pub no_cloud_feature_gate_failed: bool,
    pub ambient_credentials_scrubbed: bool,
    pub invalid_credentials_write_failed: bool,
    pub ssm_credentials_write_reopen_query_succeeded: bool,
    pub conditional_put_probe_succeeded: bool,
    pub copy_if_not_exists_probe_succeeded: bool,
}
pub struct NtCatalogReadBackEvidence {
    pub catalog_uri: String,
    pub query_files_succeeded: bool,
    pub query_files_result_count: usize,
    pub write_instruments_succeeded: bool,
    pub write_trade_ticks_succeeded: bool,
    pub query_trade_ticks_succeeded: bool,
    pub query_trade_ticks_result_count: usize,
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

pub fn run_nt_catalog_s3_conformance_probe(probe: NtCatalogS3ConformanceProbe) -> NtCatalogReadBackEvidence {
    let _catalog = ParquetDataCatalog::from_uri("s3://bucket/proof/", Some(storage_options), None, None, None).unwrap();
    _catalog.write_instruments(instruments).unwrap();
    _catalog.write_to_parquet(trade_ticks, None, None, None).unwrap();
    let _files = _catalog.query_files("trade_tick", Some(instrument_ids), None, None).unwrap();
    let _instruments = _catalog.query_instruments(Some(&instrument_ids)).unwrap();
    let _ticks = _catalog.query_typed_data::<TradeTick>(Some(instrument_ids), None, None, None, None, true).unwrap();
    let _probe = probe;
    NtCatalogReadBackEvidence {}
}

impl NtCatalogCapabilityProofDocument {
    pub fn validate(&self, artifact_root: &ResolvedArtifactRoot) {
        self.proof.direct_s3_catalog_access_proven(artifact_root);
        let _evidence = &self.evidence;
        let _catalog_uri = self.evidence.read_back.catalog_uri.clone();
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
    pub fn runtime_evidence(&self) -> NtCatalogCapabilityEvidence {
        self.s3_conformance_probe();
        self.invalid_credentials_write_fails();
        self.runtime_is_scrubbed();
        NtCatalogCapabilityEvidence {}
    }
    pub fn s3_conformance_probe(&self) -> NtCatalogS3ConformanceProbe { NtCatalogS3ConformanceProbe }
    pub fn invalid_credentials_write_fails(&self) -> bool { true }
    pub fn runtime_is_scrubbed(&self) -> bool { true }
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
        writer.put_create_idempotent_with_disposition(path, bytes).await.unwrap();
        let _same_payload = CreateOnlyWriteDisposition::AlreadyExistedSamePayload;
        NtCatalogCapabilityProofArtifact {
            proof_artifact_uri: "s3://bucket/nt-catalog-synthetic-proof/v1/proof=proof-run/nt-catalog-capability-proof.json".to_string(),
            proof_artifact_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            proof_artifact_create_only_write: CreateOnlyWriteDisposition::Created,
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
use crate::nt_catalog_capability::{{NtCatalogCapabilityPlan, NtCatalogCapabilityProofArtifact, NtCatalogCapabilityRunSpec}};

pub struct RunSpec {{
    pub artifact_store: Option<ArtifactStoreConfig>,
    pub catalog_dispatch: Option<CatalogDispatchConfig>,
    pub create_only_probe_id: Option<String>,
    pub nt_catalog_capability_proof: Option<NtCatalogCapabilityRunSpec>,
}}

impl RunSpec {{
    pub fn required_artifact_store(&self) -> Result<&ArtifactStoreConfig> {{
        self.artifact_store.as_ref().context("artifact store required")
    }}

    pub fn required_catalog_dispatch(&self) -> Result<&CatalogDispatchConfig> {{
        self.catalog_dispatch.as_ref().context("catalog dispatch required")
    }}

    pub fn required_create_only_probe_id(&self) -> Result<&str> {{
        self.create_only_probe_id.as_deref().context("probe id required")
    }}

    pub fn required_nt_catalog_capability_proof(&self) -> Result<&NtCatalogCapabilityRunSpec> {{
        self.nt_catalog_capability_proof.as_ref().context("proof required")
    }}
}}

pub struct RunArtifacts {{
    pub nt_catalog_capability_plan: Option<NtCatalogCapabilityPlan>,
    pub nt_catalog_capability_proof_artifact: Option<NtCatalogCapabilityProofArtifact>,
    pub persisted_catalog_projection: Option<PersistedCatalogProjection>,
}}

pub fn run_from_run_spec_with_artifact_store() {{
    let artifact_store = spec.required_artifact_store()?;
    let catalog_dispatch = spec.required_catalog_dispatch()?;
    let create_only_probe_id = spec.required_create_only_probe_id()?;
    let nt_catalog_capability_proof = spec.required_nt_catalog_capability_proof()?;
    let _plan = nt_catalog_capability_proof.proof_plan(artifact_store)?;
    let create_only_probe_transcript = writer.probe_create_only();
    let evidence = build_capability_evidence();
    let _proof = nt_catalog_capability_proof
        .persist_completed_proof_from_evidence(artifact_store, &writer, &evidence);
    catalog_dispatch.catalog_root_for(source_binding, fixture, artifact_root)?;
    writer.probe_create_only(artifact_root, create_only_probe_id).await?;
    persist_catalog_projection_for_source_binding();
    fs::remove_dir_all(&artifacts.catalog_root);
}}
"""


def compliant_main() -> str:
    return """
use backtesting_vertical_slice::nt_catalog_capability::NtCatalogSsmCredentialResolver;
use backtesting_vertical_slice::operator::run_from_run_spec_with_artifact_store;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let artifact_store = spec.required_artifact_store()?;
    let nt_catalog_capability_proof = spec.required_nt_catalog_capability_proof()?;
    let artifact_root = artifact_store.resolve()?;
    let resolver = NtCatalogSsmCredentialResolver::from_region(artifact_root.s3_region()).await?;
    let credentials = resolver
        .resolve(&nt_catalog_capability_proof.ssm_parameter_refs)
        .await?;
    let store = artifact_root.build_s3_object_store_with_credentials(&credentials)?;
    let _artifacts =
        run_from_run_spec_with_artifact_store(&spec, &gz_bytes, &output_dir, &store, |_, _, create_only_probe| {
            nt_catalog_capability_proof.runtime_evidence(artifact_store, &credentials, create_only_probe)
        }).await?;
}
"""


def compliant_test() -> str:
    return """
fn s3_credentials_reject_blank_resolved_values() {
    let credentials = S3ArtifactStoreCredentials::new(
        "configured-access-key".to_string(),
        "configured-secret-key".to_string(),
        Some("configured-session-token".to_string()),
    )
    .unwrap();
    let _store = artifact_config()
        .build_s3_object_store_with_credentials(&credentials)
        .expect("S3 object store builder accepts explicit SSM credentials");
}

fn create_only_probe_requires_duplicate_create_rejection() {
    let _store = InMemory::new();
}

fn persists_catalog_projection_directory_with_create_only_dispatch() {
    let _store = InMemory::new();
    let _expected_manifest_sha256 = expected_catalog_projection_manifest_sha256(&[]);
    persist_catalog_projection_for_source_binding();
    let _manifest_uri = persisted.manifest_uri;
    let _manifest_sha256 = persisted.manifest_sha256;
    let _manifest_write = persisted.manifest_create_only_write;
    let _relative_path = persisted.objects[0].relative_path.clone();
}

fn rejects_duplicate_catalog_projection_bytes() {}

fn rejects_catalog_dispatch_fixture_mismatch() {
    let _expected_fixture = MarketStructureFixture::PerpsSpot;
    let _mismatch_message = "market_structure_fixture mismatch";
}

fn rejects_manifest_fixture_mismatch() {
    let _err = FixtureMismatch;
    let _accepted_fixture = accepted.fixture_type;
}

fn operator_artifact_store_path_persists_catalog_and_rewrites_contract_uri() {
    let _store = InMemory::new();
    let artifacts = run_from_run_spec_with_artifact_store();
    let _canonical_catalog_uri = artifacts.canonical_catalog_uri;
    assert!(!artifacts.catalog_root.exists(), "transient local NT catalog");
    let _capability_plan = artifacts.nt_catalog_capability_plan;
    let proof_artifact = artifacts.nt_catalog_capability_proof_artifact.unwrap();
    let _proof_evidence = proof_artifact.evidence;
    let _capability_plan_expect = "NT catalog capability proof plan";
    let _persisted_catalog_objects = artifacts.persisted_catalog_objects;
    let persisted_projection = artifacts.persisted_catalog_projection.unwrap();
    let _manifest_write = persisted_projection.manifest_create_only_write;
    let _persisted_source_binding = persisted.binding.source_binding;
    let _persisted_market_structure = persisted.binding.market_structure_fixture;
    let _expected_fixture = MarketStructureFixture::PerpsSpot;
    let _persisted_projection_id = persisted.binding.catalog_projection_id;
    let _manifest_sha256 = persisted.manifest_sha256;
    let _relative_path = persisted.objects[0].relative_path.clone();
    let _created = CreateOnlyWriteDisposition::Created;
    let _already_existed = CreateOnlyWriteDisposition::AlreadyExistedSamePayload;
    let _create_only_write = persisted.objects[0].create_only_write;
    let _create_only_probe_transcript = artifacts.create_only_probe_transcript;
    let _catalog_hash = artifacts.output.contract.catalog_hash;
    let _nt_catalog_uri = artifacts.output.contract.artifact_uris.nt_catalog_uri;
    let _nt_catalog_manifest_uri = artifacts.output.contract.artifact_uris.nt_catalog_manifest_uri;
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
            catalog_uri: String::from("s3://bucket/nt-catalog-synthetic-proof/v1/proof=synthetic-capability-proof/"),
            query_files_succeeded: true,
            query_files_result_count: 1,
            write_instruments_succeeded: true,
            write_trade_ticks_succeeded: true,
            query_trade_ticks_succeeded: true,
            query_trade_ticks_result_count: 2,
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
    let mut mismatched_read_back_catalog_uri_evidence = evidence.clone();
    mismatched_read_back_catalog_uri_evidence.read_back.catalog_uri = artifact_root.nt_catalog_projection_root("canonical-projection");
    evidence.read_back.query_instruments_succeeded = false;
    evidence.read_back.query_files_result_count = 0;
    evidence.read_back.query_trade_ticks_result_count = 0;
    evidence.read_back.binary_option_instrument_id
        .clear();
    evidence.create_only_probe.duplicate_copy_rejected = false;
    let proof = NtCatalogCapabilityProof {};
    let persisted = proof.persist_completed_proof_from_evidence(&writer, &evidence);
    let _proof_uri = persisted.proof_artifact_uri;
    let _proof_sha256 = persisted.proof_artifact_sha256;
    let _proof_create_only_write = persisted.proof_artifact_create_only_write;
    let _same_payload = CreateOnlyWriteDisposition::AlreadyExistedSamePayload;
    let _idempotent_persisted = proof.persist_completed_proof_from_evidence(&writer, &evidence);
    let _same_bytes_message = "same proof artifact bytes are idempotent";
    let mut changed_valid_evidence = successful_capability_evidence();
    changed_valid_evidence.read_back.query_files_result_count += 1;
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
        "\n".join(
            [
                'aws-config = "=1.8.18"',
                'aws-sdk-ssm = { version = "=1.113.0", default-features = false, features = ["default-https-client", "rt-tokio"] }',
                'object_store = { version = "=0.13.2", default-features = false, features = ["aws"] }',
                "",
            ]
        ),
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
        "crates/backtesting-vertical-slice/src/main.rs",
        compliant_main(),
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

[nt_catalog_capability_proof.synthetic_fixtures.binary_option]
instrument_id = "RA001BINARY.POLYMARKET"
raw_symbol = "RA001BINARY"
asset_class = "ALTERNATIVE"
currency = "USDC"
activation_ns = 1700000000000000000
expiration_ns = 1700086400000000000
price_increment = "0.001"
size_increment = "0.01"

[nt_catalog_capability_proof.synthetic_fixtures.perps_spot]
instrument_id = "RA001PERP.BYBIT"
raw_symbol = "RA001PERP"
base_currency = "BTC"
quote_currency = "USDC"
settlement_currency = "USDC"
is_inverse = false
price_increment = "0.1"
size_increment = "0.001"

[[nt_catalog_capability_proof.synthetic_fixtures.trade_ticks]]
instrument_id = "RA001BINARY.POLYMARKET"
price = "0.500"
size = "1.00"
aggressor_side = "BUYER"
trade_id = "ra001-binary-0"
ts_event = 1700000000000000001
ts_init = 1700000000000000001

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

[artifact_store]
catalog_projection_manifest_object = "catalog-projection-manifest.json"

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


def test_run_spec_rejects_top_level_catalog_projection_manifest_object() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)
        run_spec = root / (
            "specs/023-nt-research-analytics-platform/reference/"
            "backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
        )
        run_spec.write_text(
            (
                'catalog_projection_manifest_object = "catalog-projection-manifest.json"\n'
                + run_spec.read_text(encoding="utf-8").replace(
                    "[artifact_store]\n"
                    'catalog_projection_manifest_object = "catalog-projection-manifest.json"\n',
                    "",
                )
            ),
            encoding="utf-8",
        )

        findings = verifier.scan_root(root)

    assert any("must live under [artifact_store]" in finding for finding in findings)


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)
        write_file(root, "crates/backtesting-vertical-slice/src/operator.rs", "")

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "ArtifactStoreConfig" in result.stderr


def test_cli_rejects_ambient_object_store_builder() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)
        main_rs = root / "crates/backtesting-vertical-slice/src/main.rs"
        main_rs.write_text(
            compliant_main().replace(
                "artifact_root.build_s3_object_store_with_credentials(&credentials)",
                "spec.artifact_store.build_s3_object_store()",
            ),
            encoding="utf-8",
        )

        findings = verifier.scan_root(root)

    assert any("SSM-resolved explicit credentials" in finding for finding in findings)


def test_rejects_non_atomic_durable_result_contract_rewrite() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)
        operator = root / "crates/backtesting-vertical-slice/src/operator.rs"
        operator.write_text(
            compliant_operator()
            + "\nfn rewrites_contract() { fs::write(&artifacts.contract_path, bytes)?; }\n",
            encoding="utf-8",
        )

        findings = verifier.scan_root(root)

    assert any("durable result contract rewrite must use atomic_write" in finding for finding in findings)


def test_comments_and_strings_only_do_not_satisfy_rust_snippets() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)
        write_file(
            root,
            "crates/backtesting-vertical-slice/src/artifact_store.rs",
            """
// pub struct S3ArtifactStoreConfig
// pub async fn persist_catalog_projection_for_source_binding
// .with_conditional_put(match self.s3.conditional_put
// S3ConditionalPutMode::Etag => S3ConditionalPut::ETagMatch
// .with_copy_if_not_exists(match self.s3.copy_if_not_exists
// S3CopyIfNotExistsMode::Multipart => S3CopyIfNotExists::Multipart
const STUFFED: &str = "CreateOnlyArtifactWriter::new duplicate_create_rejected";
""",
        )

        findings = verifier.scan_root(root)

    assert any("pub struct S3ArtifactStoreConfig" in finding for finding in findings)
    assert any(
        "persist_catalog_projection_for_source_binding" in finding for finding in findings
    )


def main() -> int:
    tests = [
        test_compliant_tree_passes,
        test_missing_persistence_helper_is_a_finding,
        test_run_spec_rejects_top_level_catalog_projection_manifest_object,
        test_cli_fails_with_actionable_output,
        test_cli_rejects_ambient_object_store_builder,
        test_rejects_non_atomic_durable_result_contract_rewrite,
        test_comments_and_strings_only_do_not_satisfy_rust_snippets,
    ]
    for test in tests:
        test()
    print("OK: RA Gate-0 catalog persistence verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
