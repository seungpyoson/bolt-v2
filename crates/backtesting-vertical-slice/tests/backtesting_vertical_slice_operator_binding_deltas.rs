//! End-to-end proof for the operator-binding slice over the order-book-delta
//! families: `run_operator_from_run_spec` dispatches the JSONL snapshot and tar
//! JSONL snapshot adapters through the multi-table flow, binds keyed
//! multi-instrument specs by `canonical_instrument_key`, and reuses completed
//! output byte-identically.
//!
//! Fixtures are synthetic and venue-free, built through the public source-proof
//! gate with a synthetic source-binding registry (the committed registry rejects
//! synthetic bindings), mirroring `backtesting_vertical_slice_l2_snapshot_adapter`
//! and `backtesting_vertical_slice_tar_snapshot_adapter`.

use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

use backtesting_vertical_slice::{
    canonical_order_book_deltas::{
        DeltaMappingConfig, DeltaPriceSignPolicy, DeltaSourceFormat, EmptyBookPolicy,
        InstrumentKeySpec, OrderingAuthority, SnapshotMappingFields,
    },
    canonical_trades::{
        CanonicalInstrumentIdentity, ConverterConfig, CsvTimestampUnit, CsvTradeMappingConfig,
        JSONL_SNAPSHOT_DELTAS_ADAPTER, RawPayloadConfig, RawPayloadContainer,
        SourceAdapterDefinition, TAR_JSONL_SNAPSHOT_DELTAS_ADAPTER,
    },
    catalog_projection::{CatalogInstrumentSpec, SpotInstrumentSpec},
    conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_MANIFEST_FILE,
        CONVERSION_TABLES_FILE,
    },
    hashing::sha256_hex,
    operator::{
        MultiTableRunArtifacts, OperatorRunArtifacts, RESULT_CONTRACT_FILE, RunSpec,
        RunSpecInstrumentIdentities, RunSpecInstrumentSpecs, RunSpecSelectorProvenance,
        run_operator_from_run_spec,
    },
    run_manifest::{
        BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION, BacktestingRunManifest, ManifestArtifactStore,
        ManifestCatalogInput, ManifestVenueConfig, MarketStructureFixture, RunPurpose,
        STRATEGY_HURST_VPIN_DIRECTIONAL, StrategySource, StrategySourceKind,
    },
    source_proof::{
        AcceptanceMode, AcceptanceScope, EvidenceState, FixtureType, IngestManifestObjectRecord,
        L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck, RequiredChecks,
        SourceBindingRegistry, SourceCandidateClass, SourceProofClaimLimit,
        SourceProofFidelityClass, SourceProofReport, SourceProofStatus, SourceProofUsageScope,
        SourceSelectionStatus, TimeRange,
    },
};
use flate2::{Compression, write::GzEncoder};

const NT_INSTRUMENT_ID: &str = "BASEQUOTE.TESTVENUE";
const INSTRUMENT_ID: &str = "BASEQUOTE";
const SOURCE_URL: &str = "https://synthetic.invalid/data";
const ACCEPTED_AT: &str = "2026-06-02T00:00:00Z";
const OPERATOR: &str = "operator";
const SOURCE_PROOF_ID: &str = "source-proof-synthetic-deltas";
const SOURCE_BINDING: &str = "testvenue-deltas";

const REGISTRY_TOML: &str = r#"[[source_binding]]
key = "testvenue-deltas"
venue = "testvenue"
product_family = "prediction-market"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.invalid/data"
evidence_state = "owner_archive_backfillable"
table_families = ["order_book_snapshot_deltas"]
"#;

fn registry() -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(REGISTRY_TOML)
        .expect("synthetic source binding registry parses")
}

fn write_registry(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("source-bindings.toml");
    fs::write(&path, REGISTRY_TOML).expect("write registry");
    path
}

fn spot_spec(nt_instrument_id: &str, raw_symbol: &str) -> SpotInstrumentSpec {
    SpotInstrumentSpec {
        nt_instrument_id: nt_instrument_id.to_string(),
        raw_symbol: raw_symbol.to_string(),
        base_currency: "BTC".to_string(),
        quote_currency: "USDC".to_string(),
        price_increment: "0.01".to_string(),
        size_increment: "0.001".to_string(),
        min_quantity: "0.001".to_string(),
        max_quantity: "1000000".to_string(),
        min_notional: "1".to_string(),
        max_notional: "100000000".to_string(),
    }
}

fn identity(instrument_id: &str, nt_instrument_id: &str) -> CanonicalInstrumentIdentity {
    CanonicalInstrumentIdentity {
        instrument_id: instrument_id.to_string(),
        venue_symbol: instrument_id.to_string(),
        nt_instrument_id: nt_instrument_id.to_string(),
    }
}

fn claim_limits_for(claims: &[String]) -> Vec<SourceProofClaimLimit> {
    claims
        .iter()
        .enumerate()
        .map(|(index, claim)| SourceProofClaimLimit {
            id: format!("claim-limit-{}", index + 1),
            severity: "blocking".to_string(),
            claim: claim.clone(),
            reason: "source fidelity does not prove this claim".to_string(),
            evidence_ref: "source-proof://fidelity-class".to_string(),
        })
        .collect()
}

fn accepted_proof_and_object(
    object_bytes: &[u8],
) -> (SourceProofReport, IngestManifestObjectRecord) {
    let object = IngestManifestObjectRecord {
        s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.jsonl".to_string(),
        source_url: SOURCE_URL.to_string(),
        sha256: sha256_hex(object_bytes),
        bytes: object_bytes.len() as u64,
        archive_date: "2026-05-22".to_string(),
        schema_columns: vec!["l2_snapshot_jsonl".to_string()],
    };
    let forbidden_claims = vec!["No execution-quality claims.".to_string()];
    let checks = |evidence: &str| RequiredChecks {
        source_access: RequiredCheck::passed(evidence),
        license: RequiredCheck::passed("attestation"),
        schema: RequiredCheck::passed("schema"),
        time_semantics: RequiredCheck::passed("ms_to_nanos"),
        instrument_universe: RequiredCheck::passed("universe"),
        coverage: RequiredCheck::passed(evidence),
        retention_freshness: RequiredCheck::passed("retention"),
        granularity: RequiredCheck::passed("l2_snapshot"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("OrderBookDelta"),
        cost: RequiredCheck::passed("free"),
        storage: RequiredCheck::passed("artifact_root"),
    };
    let proof = SourceProofReport {
        source_proof_id: SOURCE_PROOF_ID.to_string(),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: SOURCE_BINDING.to_string(),
        venue: "testvenue".to_string(),
        product_family: "prediction-market".to_string(),
        product_category: "binary".to_string(),
        table_family: "order_book_snapshot_deltas".to_string(),
        evidence_state: EvidenceState::OwnerArchiveBackfillable,
        source_candidate_class: SourceCandidateClass::OfficialFree,
        source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
        usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        official_free_gap_ref: None,
        paid_vendor_gap_ref: None,
        fixture_type: FixtureType::BinaryOption,
        requested_time_range: TimeRange {
            start_utc: "2025-06-01T00:00:00Z".to_string(),
            end_utc: "2026-06-01T00:00:00Z".to_string(),
        },
        coverage_time_range: TimeRange {
            start_utc: "2026-05-22T00:00:00Z".to_string(),
            end_utc: "2026-05-23T00:00:00Z".to_string(),
        },
        instrument_universe_id: "testvenue-deltas-instruments-2026-05-22".to_string(),
        raw_sample_uri: object.s3_uri.clone(),
        raw_sample_hash: object.sha256.clone(),
        schema_sample_uri: "s3://synthetic-artifacts/source-proofs/schema.json".to_string(),
        schema_sample_hash: "bf26db".to_string(),
        license_ref: "https://synthetic.invalid/ (attestation)".to_string(),
        license_scope: LicenseScope::Public,
        retention_ref: "https://synthetic.invalid/".to_string(),
        cost_ref: "cost://free-public-archive".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::L2Replay,
        l2_replay_evidence: L2ReplayEvidence {
            order_book_delta_ref: Some("source-proof://order-book-deltas".to_string()),
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: Some(
                "source-proof://no-tick-size-change-universe".to_string(),
            ),
            timed_instrument_epoch_replay_ref: None,
        },
        forbidden_claims: forbidden_claims.clone(),
        claim_limits: claim_limits_for(&forbidden_claims),
        cross_market_components: Vec::new(),
        acceptance_scope: Some(AcceptanceScope {
            planned_objects: 1,
            completed_objects: 1,
            failed_objects: 0,
            skipped_objects: 0,
            accepted_bytes: object.bytes,
            selector_scope_violations: 0,
        }),
        gap_policy_id: String::new(),
        required_checks: checks("manifest://synthetic"),
        acceptance_mode: None,
        accepted_by: None,
        accepted_at: None,
        supersedes_source_proof_id: None,
    }
    .accept_with_registry(&registry(), AcceptanceMode::Manual, OPERATOR, ACCEPTED_AT)
    .expect("accept source proof");
    (proof, object)
}

fn venue_config() -> ManifestVenueConfig {
    ManifestVenueConfig {
        nt_venue: "TESTVENUE".to_string(),
        oms_type: "NETTING".to_string(),
        account_type: "CASH".to_string(),
        book_type: "L2_MBP".to_string(),
        starting_balances: vec!["1_000_000 USDC".to_string()],
        routing: false,
        frozen_account: false,
        reject_stop_orders: true,
        support_gtd_orders: true,
        support_contingent_orders: true,
        use_position_ids: true,
        use_random_ids: false,
        use_reduce_only: true,
        bar_execution: true,
        bar_adaptive_high_low_ordering: false,
        trade_execution: true,
        use_market_order_acks: false,
        liquidity_consumption: false,
        allow_cash_borrowing: false,
        queue_position: false,
        oto_trigger_mode: "PARTIAL".to_string(),
        base_currency: "NONE".to_string(),
        default_leverage: "1".to_string(),
        price_protection_points: 0,
        leverages: None,
        margin_model: None,
        modules: None,
        fill_model: None,
        latency_model: None,
        fee_model: None,
        settlement_prices: None,
    }
}

fn delta_catalog_input(nt_instrument_id: &str) -> ManifestCatalogInput {
    ManifestCatalogInput {
        catalog_path: "overridden-by-operator-at-binding".to_string(),
        catalog_fs_protocol: "NONE".to_string(),
        catalog_fs_storage_options: BTreeMap::new(),
        catalog_fs_rust_storage_options: BTreeMap::new(),
        data_type: "OrderBookDelta".to_string(),
        nt_instrument_id: nt_instrument_id.to_string(),
        instrument_ids: None,
        start_time: None,
        end_time: None,
        filter_expr: None,
        client_id: None,
        metadata: None,
        bar_spec: None,
        bar_types: None,
        optimize_file_loading: None,
    }
}

fn manifest(
    run_id: &str,
    strategy_instrument: &str,
    catalog_inputs: Vec<ManifestCatalogInput>,
) -> BacktestingRunManifest {
    BacktestingRunManifest {
        manifest_schema_version: BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION.to_string(),
        run_id: run_id.to_string(),
        target_bolt_v2_branch: "main".to_string(),
        target_bolt_v2_ref: "refs/heads/main".to_string(),
        resolved_nt_version: "6be5a5094716790a8ca2875445fde4fa2586107e".to_string(),
        market_structure_fixture: MarketStructureFixture::BinaryOption,
        venue_binding_key: SOURCE_BINDING.to_string(),
        run_purpose: RunPurpose::Normal,
        source_proof_id: SOURCE_PROOF_ID.to_string(),
        source_proof_version: 1,
        pins_non_latest_proof: false,
        proof_pin_reason_code: None,
        proof_pin_reason_detail: None,
        strategy: StrategySource {
            source_kind: StrategySourceKind::CompiledRustRegistry,
            registry_key: STRATEGY_HURST_VPIN_DIRECTIONAL.to_string(),
            parameters: BTreeMap::from([
                ("trade_size".to_string(), "0.01".to_string()),
                (
                    "bar_type".to_string(),
                    format!("{strategy_instrument}-1-MINUTE-LAST-INTERNAL"),
                ),
            ]),
            typed_config_uri: None,
            typed_config_hash: None,
            promotion_package_uri: None,
            promotion_package_hash: None,
        },
        strategy_config_hash: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        venue: venue_config(),
        catalog_inputs,
        catalog_hash: "1111111111111111111111111111111111111111111111111111111111111111"
            .to_string(),
        execution_model: "nt_backtest_node".to_string(),
        artifact_root: "s3://synthetic-artifacts/nt-research-analytics".to_string(),
        output_prefix: format!("s3://synthetic-artifacts/nt-research-analytics/backtests/{run_id}"),
        artifact_store: ManifestArtifactStore {
            storage_options: BTreeMap::new(),
            rust_storage_options: BTreeMap::new(),
            ssm_parameters: None,
        },
        start_time: None,
        end_time: None,
    }
}

fn csv_filler_mapping() -> CsvTradeMappingConfig {
    CsvTradeMappingConfig {
        has_headers: true,
        trade_id_column: "id".to_string(),
        timestamp_column: "timestamp".to_string(),
        timestamp_unit: CsvTimestampUnit::Milliseconds,
        price_column: "price".to_string(),
        size_column: "volume".to_string(),
        side_column: "side".to_string(),
        buyer_side_values: vec!["buy".to_string()],
        seller_side_values: vec!["sell".to_string()],
    }
}

fn converter(adapter: &SourceAdapterDefinition, raw_payload: RawPayloadConfig) -> ConverterConfig {
    ConverterConfig {
        identity: adapter.identity.to_string(),
        version: adapter.version.to_string(),
        raw_payload,
        csv: csv_filler_mapping(),
        bars: None,
        paged_json_bars: None,
        jsonl_bars: None,
        deltas: None,
        quotes: None,
    }
}

fn jsonl_payload(object_len: u64) -> RawPayloadConfig {
    RawPayloadConfig {
        container: RawPayloadContainer::JsonlText,
        max_object_bytes: object_len.max(1),
        max_decoded_bytes: object_len.max(1),
        zip_member: None,
        max_member_bytes: None,
        member_suffix: None,
    }
}

fn tar_payload(object_len: u64) -> RawPayloadConfig {
    RawPayloadConfig {
        container: RawPayloadContainer::TarGzipJsonl,
        max_object_bytes: object_len.max(1),
        max_decoded_bytes: 1_048_576,
        zip_member: None,
        max_member_bytes: Some(65_536),
        member_suffix: Some(".jsonl".to_string()),
    }
}

fn snapshot_delta_mapping(key_field: Option<&str>) -> DeltaMappingConfig {
    DeltaMappingConfig {
        format: DeltaSourceFormat::Snapshot(SnapshotMappingFields {
            bids_field: "bids".to_string(),
            asks_field: "asks".to_string(),
            level_price_field: "px".to_string(),
            level_size_field: "sz".to_string(),
            event_time_field: "time".to_string(),
            event_time_unit: CsvTimestampUnit::Milliseconds,
        }),
        instrument_key: InstrumentKeySpec {
            key_field: key_field.map(ToString::to_string),
            exclusion_filter: None,
        },
        ordering: OrderingAuthority::EventTime,
        price_sign_policy: DeltaPriceSignPolicy::StrictlyPositive,
        empty_book_policy: EmptyBookPolicy::LoneClearLast,
    }
}

/// Build a delta run-spec; the caller sets `selector_provenance` afterwards
/// when the scenario requires it.
fn run_spec(
    registry_path: PathBuf,
    proof: SourceProofReport,
    object: IngestManifestObjectRecord,
    instrument_spec: RunSpecInstrumentSpecs,
    identities: RunSpecInstrumentIdentities,
    converter: ConverterConfig,
    manifest: BacktestingRunManifest,
) -> RunSpec {
    RunSpec {
        capture_time_utc: ACCEPTED_AT.to_string(),
        created_at_utc: ACCEPTED_AT.to_string(),
        accepted_by: OPERATOR.to_string(),
        accepted_at_utc: ACCEPTED_AT.to_string(),
        source_bindings_path: registry_path,
        accepted_object: object,
        source_proof: proof,
        instrument_spec,
        identity: identities,
        converter,
        manifest,
        selector_provenance: None,
    }
}

fn l2_provenance() -> Option<RunSpecSelectorProvenance> {
    Some(RunSpecSelectorProvenance {
        event_count_ledger_hash: "eventledgerabc".to_string(),
        selected_asset_ids_hash: "selectedassetsabc".to_string(),
    })
}

fn assert_multi(artifacts: OperatorRunArtifacts) -> MultiTableRunArtifacts {
    match artifacts {
        OperatorRunArtifacts::MultiTable(artifacts) => *artifacts,
        OperatorRunArtifacts::Trade(_) => {
            panic!("non-trade adapter kinds must dispatch through the multi-table flow")
        }
    }
}

fn read_artifact_bytes(dir: &std::path::Path, name: &str) -> Vec<u8> {
    fs::read(dir.join(name)).unwrap_or_else(|error| panic!("read artifact {name}: {error}"))
}

/// Run a second time on the same output dir and prove the reuse path keeps
/// every completion artifact byte-identical (and the tables index when present).
fn assert_idempotent_rerun(spec: &RunSpec, object_bytes: &[u8], dir: &std::path::Path) {
    let mut before = BTreeMap::new();
    for name in [
        CONVERSION_MANIFEST_FILE,
        CONVERSION_CHECKPOINT_FILE,
        CATALOG_METADATA_FILE,
        RESULT_CONTRACT_FILE,
    ] {
        before.insert(name, read_artifact_bytes(dir, name));
    }
    let tables_before = dir
        .join(CONVERSION_TABLES_FILE)
        .exists()
        .then(|| read_artifact_bytes(dir, CONVERSION_TABLES_FILE));

    let rerun = run_operator_from_run_spec(spec, object_bytes, dir).expect("idempotent rerun");
    let rerun = assert_multi(rerun);
    assert_eq!(
        rerun.conversion_tables_path.is_some(),
        tables_before.is_some(),
        "rerun must preserve tables-index presence"
    );
    for (name, bytes) in before {
        assert_eq!(
            read_artifact_bytes(dir, name),
            bytes,
            "completed artifact {name} must stay byte-identical across reruns"
        );
    }
    if let Some(tables_bytes) = tables_before {
        assert_eq!(
            read_artifact_bytes(dir, CONVERSION_TABLES_FILE),
            tables_bytes,
            "conversion tables index must stay byte-identical across reruns"
        );
    }
}

const TAR_BLOCK: usize = 512;

fn ustar_header(name: &str, size: u64) -> [u8; TAR_BLOCK] {
    let mut header = [0u8; TAR_BLOCK];
    let name_bytes = name.as_bytes();
    assert!(name_bytes.len() <= 100, "test member name too long");
    header[0..name_bytes.len()].copy_from_slice(name_bytes);
    header[100..107].copy_from_slice(b"0000644");
    header[108..115].copy_from_slice(b"0000000");
    header[116..123].copy_from_slice(b"0000000");
    let size_field = format!("{size:011o}");
    header[124..135].copy_from_slice(size_field.as_bytes());
    header[135] = b' ';
    header[136..147].copy_from_slice(b"00000000000");
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    header[148..156].copy_from_slice(b"        ");
    let checksum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
    let checksum_field = format!("{checksum:06o}");
    header[148..154].copy_from_slice(checksum_field.as_bytes());
    header[154] = 0;
    header[155] = b' ';
    header
}

fn gzip_tar(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, data) in members {
        tar.extend_from_slice(&ustar_header(name, data.len() as u64));
        tar.extend_from_slice(data);
        let padding = (TAR_BLOCK - data.len() % TAR_BLOCK) % TAR_BLOCK;
        tar.extend(std::iter::repeat_n(0u8, padding));
    }
    tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

#[test]
fn jsonl_snapshot_deltas_run_spec_end_to_end() {
    let jsonl = "{\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"},{\"px\":\"0.48\",\"sz\":\"7\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
        {\"time\":1700000060000,\"bids\":[{\"px\":\"0.50\",\"sz\":\"11\"},{\"px\":\"0.49\",\"sz\":\"6\"}],\"asks\":[{\"px\":\"0.52\",\"sz\":\"13\"}]}\n";
    let object_bytes = jsonl.as_bytes();
    let (proof, object) = accepted_proof_and_object(object_bytes);
    let temp = tempfile::TempDir::new().expect("temp dir");
    let registry_path = write_registry(temp.path());
    let mut converter = converter(
        &JSONL_SNAPSHOT_DELTAS_ADAPTER,
        jsonl_payload(object_bytes.len() as u64),
    );
    converter.deltas = Some(snapshot_delta_mapping(None));
    let manifest = manifest(
        "operator-binding-jsonl-deltas",
        NT_INSTRUMENT_ID,
        vec![delta_catalog_input(NT_INSTRUMENT_ID)],
    );
    let mut spec = run_spec(
        registry_path,
        proof,
        object,
        RunSpecInstrumentSpecs::Single(Box::new(CatalogInstrumentSpec::Spot(spot_spec(
            NT_INSTRUMENT_ID,
            INSTRUMENT_ID,
        )))),
        RunSpecInstrumentIdentities::Single(identity(INSTRUMENT_ID, NT_INSTRUMENT_ID)),
        converter,
        manifest,
    );
    spec.selector_provenance = l2_provenance();

    let output_dir = temp.path().join("out");
    let artifacts = assert_multi(
        run_operator_from_run_spec(&spec, object_bytes, &output_dir).expect("operator run"),
    );

    assert_eq!(artifacts.tables.len(), 1);
    let table = &artifacts.tables[0];
    assert_eq!(table.table_family, "order_book_snapshot_deltas");
    assert_eq!(table.data_type, "OrderBookDelta");
    assert_eq!(table.bar_spec, None);
    assert_eq!(
        table.subroot_relative,
        "nt-catalogs/order_book_snapshot_deltas/BASEQUOTE_TESTVENUE/default"
    );
    assert!(table.rows > 0);
    assert!(table.canonical_path.is_file(), "canonical parquet exists");
    assert_eq!(artifacts.nt_result.iterations, table.rows);
    assert_eq!(
        artifacts.contract.fidelity_class,
        SourceProofFidelityClass::L2Replay
    );
    assert_eq!(
        artifacts.contract.event_count_ledger_hash.as_deref(),
        Some("eventledgerabc")
    );
    assert!(
        artifacts.conversion_tables_path.is_none(),
        "single-table conversions never write the tables index"
    );

    assert_idempotent_rerun(&spec, object_bytes, &output_dir);
}

#[test]
fn tar_jsonl_snapshot_deltas_run_spec_end_to_end_multi_member() {
    // Two members in archive order; the photo stream continues across members.
    let member_one = "{\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
    let member_two = "{\"time\":1700000060000,\"bids\":[{\"px\":\"0.50\",\"sz\":\"11\"}],\"asks\":[{\"px\":\"0.52\",\"sz\":\"13\"}]}\n";
    let object_bytes = gzip_tar(&[
        ("0000.jsonl", member_one.as_bytes()),
        ("0001.jsonl", member_two.as_bytes()),
    ]);
    let (proof, object) = accepted_proof_and_object(&object_bytes);
    let temp = tempfile::TempDir::new().expect("temp dir");
    let registry_path = write_registry(temp.path());
    let mut converter = converter(
        &TAR_JSONL_SNAPSHOT_DELTAS_ADAPTER,
        tar_payload(object_bytes.len() as u64),
    );
    converter.deltas = Some(snapshot_delta_mapping(None));
    let manifest = manifest(
        "operator-binding-tar-deltas",
        NT_INSTRUMENT_ID,
        vec![delta_catalog_input(NT_INSTRUMENT_ID)],
    );
    let mut spec = run_spec(
        registry_path,
        proof,
        object,
        RunSpecInstrumentSpecs::Single(Box::new(CatalogInstrumentSpec::Spot(spot_spec(
            NT_INSTRUMENT_ID,
            INSTRUMENT_ID,
        )))),
        RunSpecInstrumentIdentities::Single(identity(INSTRUMENT_ID, NT_INSTRUMENT_ID)),
        converter,
        manifest,
    );
    spec.selector_provenance = l2_provenance();

    let output_dir = temp.path().join("out");
    let artifacts = assert_multi(
        run_operator_from_run_spec(&spec, &object_bytes, &output_dir).expect("operator run"),
    );

    assert_eq!(artifacts.tables.len(), 1);
    let table = &artifacts.tables[0];
    assert_eq!(table.table_family, "order_book_snapshot_deltas");
    assert!(
        table.rows > 2,
        "two photos expand to more than two deltas, got {}",
        table.rows
    );
    assert_eq!(artifacts.nt_result.iterations, table.rows);

    assert_idempotent_rerun(&spec, &object_bytes, &output_dir);
}

#[test]
fn tar_snapshot_keyed_specs_two_instruments_two_subroots() {
    let member_one = "{\"coin\":\"AAA\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
        {\"coin\":\"BBB\",\"time\":1700000001000,\"bids\":[{\"px\":\"0.39\",\"sz\":\"20\"}],\"asks\":[{\"px\":\"0.41\",\"sz\":\"22\"}]}\n";
    let member_two = "{\"coin\":\"AAA\",\"time\":1700000060000,\"bids\":[{\"px\":\"0.50\",\"sz\":\"11\"}],\"asks\":[{\"px\":\"0.52\",\"sz\":\"13\"}]}\n\
        {\"coin\":\"BBB\",\"time\":1700000061000,\"bids\":[{\"px\":\"0.40\",\"sz\":\"21\"}],\"asks\":[{\"px\":\"0.42\",\"sz\":\"23\"}]}\n";
    let object_bytes = gzip_tar(&[
        ("0000.jsonl", member_one.as_bytes()),
        ("0001.jsonl", member_two.as_bytes()),
    ]);
    let (proof, object) = accepted_proof_and_object(&object_bytes);
    let temp = tempfile::TempDir::new().expect("temp dir");
    let registry_path = write_registry(temp.path());
    let mut converter = converter(
        &TAR_JSONL_SNAPSHOT_DELTAS_ADAPTER,
        tar_payload(object_bytes.len() as u64),
    );
    converter.deltas = Some(snapshot_delta_mapping(Some("coin")));
    let manifest = manifest(
        "operator-binding-tar-deltas-keyed",
        "AAA.TESTVENUE",
        vec![
            delta_catalog_input("AAA.TESTVENUE"),
            delta_catalog_input("BBB.TESTVENUE"),
        ],
    );
    let mut spec = run_spec(
        registry_path,
        proof,
        object,
        RunSpecInstrumentSpecs::Keyed(BTreeMap::from([
            (
                "testvenue/prediction-market/AAA".to_string(),
                CatalogInstrumentSpec::Spot(spot_spec("AAA.TESTVENUE", "AAA")),
            ),
            (
                "testvenue/prediction-market/BBB".to_string(),
                CatalogInstrumentSpec::Spot(spot_spec("BBB.TESTVENUE", "BBB")),
            ),
        ])),
        RunSpecInstrumentIdentities::Keyed(BTreeMap::from([
            ("AAA".to_string(), identity("AAA", "AAA.TESTVENUE")),
            ("BBB".to_string(), identity("BBB", "BBB.TESTVENUE")),
        ])),
        converter,
        manifest,
    );
    spec.selector_provenance = l2_provenance();

    let output_dir = temp.path().join("out");
    let artifacts = assert_multi(
        run_operator_from_run_spec(&spec, &object_bytes, &output_dir).expect("operator run"),
    );

    assert_eq!(artifacts.tables.len(), 2, "two instruments -> two tables");
    let aaa = artifacts
        .tables
        .iter()
        .find(|table| table.nt_instrument_id == "AAA.TESTVENUE")
        .expect("AAA table present");
    let bbb = artifacts
        .tables
        .iter()
        .find(|table| table.nt_instrument_id == "BBB.TESTVENUE")
        .expect("BBB table present");
    assert_eq!(
        aaa.subroot_relative,
        "nt-catalogs/order_book_snapshot_deltas/AAA_TESTVENUE/default"
    );
    assert_eq!(
        bbb.subroot_relative,
        "nt-catalogs/order_book_snapshot_deltas/BBB_TESTVENUE/default"
    );
    assert_eq!(artifacts.nt_result.iterations, aaa.rows + bbb.rows);
    assert!(
        artifacts.conversion_tables_path.is_some(),
        "two tables -> tables index written"
    );

    assert_idempotent_rerun(&spec, &object_bytes, &output_dir);
}

#[test]
fn keyed_spec_missing_canonical_key_fails_loud() {
    let member = "{\"coin\":\"AAA\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
        {\"coin\":\"BBB\",\"time\":1700000001000,\"bids\":[{\"px\":\"0.39\",\"sz\":\"20\"}],\"asks\":[{\"px\":\"0.41\",\"sz\":\"22\"}]}\n";
    let object_bytes = gzip_tar(&[("0000.jsonl", member.as_bytes())]);
    let (proof, object) = accepted_proof_and_object(&object_bytes);
    let temp = tempfile::TempDir::new().expect("temp dir");
    let registry_path = write_registry(temp.path());
    let mut converter = converter(
        &TAR_JSONL_SNAPSHOT_DELTAS_ADAPTER,
        tar_payload(object_bytes.len() as u64),
    );
    converter.deltas = Some(snapshot_delta_mapping(Some("coin")));
    let manifest = manifest(
        "operator-binding-tar-deltas-missing-key",
        "AAA.TESTVENUE",
        vec![
            delta_catalog_input("AAA.TESTVENUE"),
            delta_catalog_input("BBB.TESTVENUE"),
        ],
    );
    // The keyed spec map deliberately misses the BBB canonical key.
    let mut spec = run_spec(
        registry_path,
        proof,
        object,
        RunSpecInstrumentSpecs::Keyed(BTreeMap::from([(
            "testvenue/prediction-market/AAA".to_string(),
            CatalogInstrumentSpec::Spot(spot_spec("AAA.TESTVENUE", "AAA")),
        )])),
        RunSpecInstrumentIdentities::Keyed(BTreeMap::from([
            ("AAA".to_string(), identity("AAA", "AAA.TESTVENUE")),
            ("BBB".to_string(), identity("BBB", "BBB.TESTVENUE")),
        ])),
        converter,
        manifest,
    );
    spec.selector_provenance = l2_provenance();

    let err = run_operator_from_run_spec(&spec, &object_bytes, &temp.path().join("out"))
        .err()
        .expect("missing keyed instrument spec must fail loud");
    assert!(
        err.to_string()
            .contains("no entry for canonical_instrument_key"),
        "{err}"
    );
    assert!(
        err.to_string().contains("testvenue/prediction-market/BBB"),
        "{err}"
    );
}
