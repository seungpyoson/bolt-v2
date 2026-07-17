//! End-to-end proof for the config-driven CSV bar source adapter (format family
//! F1).
//!
//! Proves, against the NautilusTrader dependency resolved by this `bolt-v2`
//! branch, that an accepted CSV bar object normalizes through
//! [`normalize_csv_native_bars`] into a validated [`CanonicalBarsTable`] and
//! projects into a local `ParquetDataCatalog` as externally-aggregated `Bar`
//! data that reads back with `ts_event == close_time` and numeric OHLCV
//! equality, and that a single-member ZIP envelope decodes to the same
//! normalized table.
//!
//! Fixtures are synthetic and venue-free: the adapter is data-driven and must
//! not be tied to any real venue, token, symbol, or incident value. The
//! accepted dataset is built through the public source-proof gate with a
//! synthetic source-binding registry, since [`AcceptedDataset`] cannot be
//! constructed outside that gate.

use std::io::{Cursor, Read, Write};

use backtesting_vertical_slice::{
    canonical_bars::{
        BarInstrumentIdentities, BarIntervalSource, BarMappingConfig, BarPriceSignPolicy,
        normalize_csv_native_bars,
    },
    canonical_trades::{CanonicalInstrumentIdentity, CsvTimestampUnit},
    catalog_projection::{SpotInstrumentSpec, project_canonical_bars_to_catalog, read_back_bars},
    source_proof::{
        AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
        IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck,
        RequiredChecks, SourceBindingRegistry, SourceCandidateClass, SourceProofClaimLimit,
        SourceProofFidelityClass, SourceProofReport, SourceProofStatus, SourceProofUsageScope,
        SourceSelectionStatus, TimeRange, select_accepted_dataset_with_registry,
    },
};
use nautilus_model::{
    enums::BarAggregation,
    types::{Price, Quantity},
};

const NT_INSTRUMENT_ID: &str = "BASEQUOTE.TESTVENUE";
const INSTRUMENT_ID: &str = "BASEQUOTE";
const OBJECT_SHA256: &str = "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598";

fn test_catalog_encoding() -> backtesting_vertical_slice::artifact_store::CatalogEncodingConfig {
    backtesting_vertical_slice::artifact_store::CatalogEncodingConfig::new(
        5000,
        5000,
        backtesting_vertical_slice::artifact_store::CatalogCompression::Snappy,
    )
    .expect("positive test catalog encoding")
}
const SOURCE_URL: &str = "https://synthetic.invalid/data";

// open_time/close_time are unix-ms; the period is one minute. close_time is the
// bar's event instant and must equal the read-back `ts_event`.
const BARS_CSV: &str = "open_time,close_time,open,high,low,close,volume\n\
    1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
    1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n";

const SCHEMA_COLUMNS: [&str; 7] = [
    "open_time",
    "close_time",
    "open",
    "high",
    "low",
    "close",
    "volume",
];

fn spec() -> SpotInstrumentSpec {
    SpotInstrumentSpec {
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        raw_symbol: INSTRUMENT_ID.to_string(),
        base_currency: "BASE".to_string(),
        quote_currency: "QUOTE".to_string(),
        price_increment: "0.01".to_string(),
        size_increment: "0.001".to_string(),
        min_quantity: "0.001".to_string(),
        max_quantity: "1000000".to_string(),
        min_notional: "1".to_string(),
        max_notional: "100000000".to_string(),
    }
}

fn identities() -> BarInstrumentIdentities {
    BarInstrumentIdentities::Single(CanonicalInstrumentIdentity {
        instrument_id: INSTRUMENT_ID.to_string(),
        venue_symbol: INSTRUMENT_ID.to_string(),
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
    })
}

fn mapping() -> BarMappingConfig {
    BarMappingConfig {
        has_headers: true,
        open_time_column: "open_time".to_string(),
        close_time_column: "close_time".to_string(),
        timestamp_unit: CsvTimestampUnit::Milliseconds,
        open_column: "open".to_string(),
        high_column: "high".to_string(),
        low_column: "low".to_string(),
        close_column: "close".to_string(),
        volume_column: "volume".to_string(),
        instrument_column: None,
        interval_source: BarIntervalSource::Declared {
            step: 1,
            aggregation: BarAggregation::Minute,
        },
        price_sign_policy: BarPriceSignPolicy::StrictlyPositive,
    }
}

fn source_binding_registry() -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(
        r#"[[source_binding]]
key = "testvenue-bars"
venue = "testvenue"
product_family = "prediction-market"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.invalid/data"
evidence_state = "owner_archive_backfillable"
table_families = ["bars"]
"#,
    )
    .expect("synthetic source binding registry parses")
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

fn accepted_dataset() -> AcceptedDataset {
    let object = IngestManifestObjectRecord {
        s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.csv".to_string(),
        source_url: SOURCE_URL.to_string(),
        sha256: OBJECT_SHA256.to_string(),
        bytes: 4096,
        archive_date: "2026-05-22".to_string(),
        schema_columns: SCHEMA_COLUMNS.iter().map(ToString::to_string).collect(),
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
        granularity: RequiredCheck::passed("aggregated_bars"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("Bar"),
        cost: RequiredCheck::passed("free"),
        storage: RequiredCheck::passed("artifact_root"),
    };
    let proof = SourceProofReport {
        source_proof_id: "source-proof-synthetic-bars".to_string(),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: "testvenue-bars".to_string(),
        venue: "testvenue".to_string(),
        product_family: "prediction-market".to_string(),
        product_category: "binary".to_string(),
        table_family: "bars".to_string(),
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
        instrument_universe_id: "testvenue-bars-instruments-2026-05-22".to_string(),
        raw_sample_uri: object.s3_uri.clone(),
        raw_sample_hash: object.sha256.clone(),
        schema_sample_uri: "s3://synthetic-artifacts/source-proofs/schema.json".to_string(),
        schema_sample_hash: "bf26db".to_string(),
        license_ref: "https://synthetic.invalid/ (attestation)".to_string(),
        license_scope: LicenseScope::Public,
        retention_ref: "https://synthetic.invalid/".to_string(),
        cost_ref: "cost://free-public-archive".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::TradeBarReplay,
        l2_replay_evidence: L2ReplayEvidence {
            order_book_delta_ref: None,
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: None,
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
    .accept_with_registry(
        &source_binding_registry(),
        AcceptanceMode::Manual,
        "operator",
        "2026-06-02T00:00:00Z",
    )
    .expect("accept source proof");
    select_accepted_dataset_with_registry(
        &proof,
        &object,
        &object.sha256,
        &source_binding_registry(),
    )
    .expect("select accepted dataset")
}

#[test]
fn csv_native_bars_round_trip_to_catalog() {
    let accepted = accepted_dataset();
    let tables = normalize_csv_native_bars(
        &accepted,
        &identities(),
        &mapping(),
        BARS_CSV,
        42,
        "ingest-run-test",
    )
    .expect("normalize csv bars");
    assert_eq!(tables.len(), 1, "single-instrument object yields one table");
    let table = &tables[0];
    assert_eq!(table.rows.len(), 2);

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection =
        project_canonical_bars_to_catalog(table, &spec(), dir.path(), &test_catalog_encoding())
            .expect("project bars");
    assert_eq!(projection.trade_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(
        projection.fidelity_class,
        SourceProofFidelityClass::TradeBarReplay
    );

    let mut loaded = read_back_bars(dir.path(), NT_INSTRUMENT_ID).expect("read back bars");
    assert_eq!(loaded.len(), table.rows.len());
    loaded.sort_by_key(|bar| bar.ts_event.as_u64());
    for (bar, row) in loaded.iter().zip(table.rows.iter()) {
        assert_eq!(bar.instrument_id().to_string(), NT_INSTRUMENT_ID);
        // ts_event is the canonical close_time (the bar's event instant); ts_init
        // is the receipt clock NautilusTrader replays by, which for this native
        // CSV source (no availability column) is the row's capture_time.
        assert_eq!(bar.ts_event.as_u64(), row.close_time as u64);
        assert_eq!(bar.ts_init.as_u64(), row.capture_time as u64);
        // Compare OHLCV numerically: Display renders at instrument precision.
        assert_eq!(
            bar.open.as_decimal(),
            Price::from(row.open.as_str()).as_decimal()
        );
        assert_eq!(
            bar.high.as_decimal(),
            Price::from(row.high.as_str()).as_decimal()
        );
        assert_eq!(
            bar.low.as_decimal(),
            Price::from(row.low.as_str()).as_decimal()
        );
        assert_eq!(
            bar.close.as_decimal(),
            Price::from(row.close.as_str()).as_decimal()
        );
        assert_eq!(
            bar.volume.as_decimal(),
            Quantity::from(row.volume.as_str()).as_decimal()
        );
    }
}

#[test]
fn csv_native_bars_in_zip_decodes_and_normalizes() {
    let zip_bytes = zip_single_csv("bars.csv", BARS_CSV);
    let csv_text = decode_single_member_zip(&zip_bytes, "bars.csv");

    let accepted = accepted_dataset();
    let tables = normalize_csv_native_bars(
        &accepted,
        &identities(),
        &mapping(),
        &csv_text,
        42,
        "ingest-run-test",
    )
    .expect("normalize bars decoded from zip");
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].rows.len(), 2);
    assert_eq!(tables[0].rows[0].open_time, 1_700_000_000_000_000_000);
    assert_eq!(tables[0].rows[0].close_time, 1_700_000_060_000_000_000);
}

fn zip_single_csv(member_name: &str, text: &str) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file(member_name, zip::write::FileOptions::default())
        .expect("start zip member");
    writer.write_all(text.as_bytes()).expect("write zip member");
    writer.finish().expect("finish zip").into_inner()
}

fn decode_single_member_zip(zip_bytes: &[u8], member_name: &str) -> String {
    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor).expect("open zip object");
    let mut member = archive.by_name(member_name).expect("open zip member");
    assert!(!member.is_dir(), "zip member must not be a directory");
    let mut text = String::new();
    member.read_to_string(&mut text).expect("read zip member");
    text
}
