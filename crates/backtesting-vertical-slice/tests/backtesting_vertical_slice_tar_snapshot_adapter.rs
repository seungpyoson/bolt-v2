//! End-to-end proof for the streaming tar-of-JSONL periodic-full-snapshot L2
//! delta source adapter (format family S3, tar-bundled container).
//!
//! Proves, against the NautilusTrader dependency resolved by this `bolt-v2`
//! branch, that a synthetic gzip-tar of JSONL members streams through
//! [`normalize_tar_jsonl_snapshot_deltas`] into validated
//! [`CanonicalOrderBookDeltasTable`]s — one per instrument, with grouping
//! spanning members — and projects into a local `ParquetDataCatalog` as
//! `OrderBookDelta` data that reads back with per-field equality. A second test
//! proves the per-member byte bound is independent of the cumulative archive
//! bound: many small members pass when each member and the whole archive remain
//! inside their separately configured limits.
//!
//! This is the tar-container sibling of the JSONL `l2_snapshot_adapter` proof and
//! exists to kill two defects of the superseded converter lane: gunzipping a
//! whole multi-gibibyte tar into memory, and processing only the first member.
//!
//! Fixtures are synthetic and venue-free: the adapter is data-driven and must not
//! be tied to any real venue, token, symbol, or incident value. The accepted
//! dataset is built through the public source-proof gate with a synthetic
//! source-binding registry, since [`AcceptedDataset`] cannot be constructed
//! outside that gate.

use std::collections::BTreeMap;
use std::io::Write;

use backtesting_vertical_slice::{
    canonical_market_data::{DeltaAction, DeltaSide},
    canonical_order_book_deltas::{
        DeltaInstrumentIdentities, DeltaMappingConfig, DeltaPriceSignPolicy, DeltaSourceFormat,
        EmptyBookPolicy, InstrumentExclusionFilter, InstrumentKeySpec, OrderingAuthority,
        SnapshotMappingFields, normalize_tar_jsonl_snapshot_deltas,
    },
    canonical_trades::{CanonicalInstrumentIdentity, CsvTimestampUnit},
    catalog_projection::{
        DeltaReplayClock, SpotInstrumentSpec, project_canonical_order_book_deltas_to_catalog,
        read_back_order_book_deltas,
    },
    jsonl_record_stream::JsonlStreamLimits,
    source_proof::{
        AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
        IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck,
        RequiredChecks, SourceBindingRegistry, SourceCandidateClass, SourceProofClaimLimit,
        SourceProofFidelityClass, SourceProofReport, SourceProofStatus, SourceProofUsageScope,
        SourceSelectionStatus, TimeRange, select_accepted_dataset_with_registry,
    },
};
use flate2::{Compression, write::GzEncoder};
use nautilus_model::{
    enums::{BookAction, OrderSide},
    types::{Price, Quantity},
};

const OBJECT_SHA256: &str = "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598";
const SOURCE_URL: &str = "https://synthetic.invalid/data";

/// POSIX tar block size (header + data are laid out in 512-byte blocks).
const TAR_BLOCK: usize = 512;

fn stream_limits(max_decoded_bytes: u64, max_member_bytes: u64) -> JsonlStreamLimits {
    JsonlStreamLimits {
        max_decoded_bytes,
        max_members: max_decoded_bytes,
        max_member_bytes,
        max_record_bytes: usize::try_from(max_member_bytes).expect("test member bound fits usize"),
        max_records: max_decoded_bytes,
        member_suffix: Some(".data".to_string()),
    }
}

/// One synthetic instrument's NT/raw identity pair.
struct Instrument {
    key: &'static str,
    raw: &'static str,
    nt: &'static str,
}

const INSTRUMENT_ONE: Instrument = Instrument {
    key: "AAA",
    raw: "BASEONE",
    nt: "BASEONE.TESTVENUE",
};

const INSTRUMENT_TWO: Instrument = Instrument {
    key: "BBB",
    raw: "BASETWO",
    nt: "BASETWO.TESTVENUE",
};

fn spec(instrument: &Instrument) -> SpotInstrumentSpec {
    SpotInstrumentSpec {
        nt_instrument_id: instrument.nt.to_string(),
        raw_symbol: instrument.raw.to_string(),
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

fn keyed_identities() -> DeltaInstrumentIdentities {
    DeltaInstrumentIdentities::Keyed(BTreeMap::from([
        (
            INSTRUMENT_ONE.key.to_string(),
            CanonicalInstrumentIdentity {
                instrument_id: INSTRUMENT_ONE.raw.to_string(),
                venue_symbol: INSTRUMENT_ONE.raw.to_string(),
                nt_instrument_id: INSTRUMENT_ONE.nt.to_string(),
            },
        ),
        (
            INSTRUMENT_TWO.key.to_string(),
            CanonicalInstrumentIdentity {
                instrument_id: INSTRUMENT_TWO.raw.to_string(),
                venue_symbol: INSTRUMENT_TWO.raw.to_string(),
                nt_instrument_id: INSTRUMENT_TWO.nt.to_string(),
            },
        ),
    ]))
}

fn keyed_mapping() -> DeltaMappingConfig {
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
            key_field: Some("coin".to_string()),
            exclusion_filter: Some(InstrumentExclusionFilter::default()),
        },
        ordering: OrderingAuthority::EventTime,
        price_sign_policy: DeltaPriceSignPolicy::StrictlyPositive,
        empty_book_policy: EmptyBookPolicy::LoneClearLast,
    }
}

fn source_binding_registry() -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(
        r#"[[source_binding]]
key = "testvenue-deltas"
venue = "testvenue"
product_family = "prediction-market"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.invalid/data"
evidence_state = "owner_archive_backfillable"
table_families = ["order_book_snapshot_deltas"]
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
        s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.tar.gz".to_string(),
        source_url: SOURCE_URL.to_string(),
        sha256: OBJECT_SHA256.to_string(),
        bytes: 4096,
        archive_date: "2026-05-22".to_string(),
        schema_columns: vec!["l2_snapshot_tar_jsonl".to_string()],
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
        source_proof_id: "source-proof-synthetic-deltas".to_string(),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: "testvenue-deltas".to_string(),
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

/// Build a minimal POSIX `ustar` header block for one regular-file member.
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

/// Append one member (header + data + block padding) to a raw tar buffer.
fn push_member(tar: &mut Vec<u8>, name: &str, data: &[u8]) {
    tar.extend_from_slice(&ustar_header(name, data.len() as u64));
    tar.extend_from_slice(data);
    let padding = (TAR_BLOCK - data.len() % TAR_BLOCK) % TAR_BLOCK;
    tar.extend(std::iter::repeat_n(0u8, padding));
}

/// Gzip a sequence of named `.data` members into a complete tar archive
/// (members + two zero end-of-archive blocks).
fn gzip_tar(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, data) in members {
        push_member(&mut tar, name, data);
    }
    tar.extend(std::iter::repeat_n(0u8, TAR_BLOCK * 2));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

/// One photo line for `instrument` at `time_ms` with one bid + one ask.
fn photo_line(key: &str, time_ms: i64, bid_px: &str, ask_px: &str) -> String {
    format!(
        "{{\"coin\":\"{key}\",\"time\":{time_ms},\"bids\":[{{\"px\":\"{bid_px}\",\"sz\":\"10\"}}],\"asks\":[{{\"px\":\"{ask_px}\",\"sz\":\"12\"}}]}}\n"
    )
}

#[test]
fn tar_snapshot_deltas_split_across_members_round_trip_to_catalog() {
    // Three members. Instrument ONE appears in members 0 and 2 (proving its
    // grouping spans non-adjacent members); instrument TWO appears only in
    // member 1. The two ONE photos are also out of archive order across the
    // members (later time in the earlier member) to exercise the stable
    // event-time sort.
    let member_0 = format!(
        "{}{}",
        photo_line(INSTRUMENT_ONE.key, 1_700_000_060_000, "0.50", "0.52"),
        photo_line(INSTRUMENT_TWO.key, 1_700_000_000_000, "0.30", "0.33"),
    );
    let member_1 = photo_line(INSTRUMENT_TWO.key, 1_700_000_060_000, "0.31", "0.34");
    let member_2 = photo_line(INSTRUMENT_ONE.key, 1_700_000_000_000, "0.49", "0.51");

    let archive = gzip_tar(&[
        ("000.data", member_0.as_bytes()),
        ("001.data", member_1.as_bytes()),
        ("002.data", member_2.as_bytes()),
    ]);

    let accepted = accepted_dataset();
    let limits = stream_limits(1 << 20, 1 << 20);
    let mut tables = normalize_tar_jsonl_snapshot_deltas(
        &accepted,
        &keyed_identities(),
        &keyed_mapping(),
        &archive,
        &limits,
        42,
        "ingest-run-test",
    )
    .expect("normalize tar of jsonl snapshots");
    tables.sort_by(|left, right| {
        left.partition
            .instrument_id
            .cmp(&right.partition.instrument_id)
    });
    assert_eq!(tables.len(), 2, "two instruments split across members");
    assert_eq!(tables[0].partition.instrument_id, INSTRUMENT_ONE.raw);
    assert_eq!(tables[1].partition.instrument_id, INSTRUMENT_TWO.raw);

    // Each instrument carries two photos = two (CLEAR + bid ADD + ask ADD) = 6
    // rows, and the per-instrument timeline is monotonic after the sort.
    for table in &tables {
        assert_eq!(table.rows.len(), 6);
        assert_eq!(table.rows[0].event_time, 1_700_000_000_000_000_000);
        assert_eq!(table.rows[3].event_time, 1_700_000_060_000_000_000);
    }

    // Project both instruments and read back per-field for instrument ONE,
    // proving the cross-member group projects faithfully through the catalog.
    let table_one = &tables[0];
    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection = project_canonical_order_book_deltas_to_catalog(
        table_one,
        &spec(&INSTRUMENT_ONE),
        DeltaReplayClock::SourceAvailability,
        dir.path(),
    )
    .expect("project instrument one");
    assert_eq!(projection.trade_count, table_one.rows.len());
    assert_eq!(projection.nt_instrument_id, INSTRUMENT_ONE.nt);
    assert_eq!(
        projection.fidelity_class,
        SourceProofFidelityClass::L2Replay
    );

    let loaded =
        read_back_order_book_deltas(dir.path(), INSTRUMENT_ONE.nt).expect("read back deltas");
    assert_eq!(loaded.len(), table_one.rows.len());
    for (delta, row) in loaded.iter().zip(table_one.rows.iter()) {
        assert_eq!(delta.instrument_id.to_string(), INSTRUMENT_ONE.nt);
        assert_eq!(delta.sequence, 0, "source carries no native sequence");
        assert_eq!(delta.flags, row.flags);
        assert_eq!(delta.ts_event.as_u64(), row.event_time as u64);
        if row.action == DeltaAction::Clear.as_str() {
            assert_eq!(delta.action, BookAction::Clear);
        } else {
            assert_eq!(delta.action, BookAction::Add);
            assert_eq!(
                delta.order.price.as_decimal(),
                Price::from(row.price.as_str()).as_decimal()
            );
            assert_eq!(
                delta.order.size.as_decimal(),
                Quantity::from(row.size.as_str()).as_decimal()
            );
            let expected_side = if row.side == DeltaSide::Buy.as_str() {
                OrderSide::Buy
            } else {
                OrderSide::Sell
            };
            assert_eq!(delta.order.side, expected_side);
        }
    }
}

#[test]
fn many_small_members_pass_under_a_small_per_member_bound() {
    // Memory-discipline regression: 64 single-photo members. Each member's text
    // is well under the bound, but the cumulative decoded size of all members
    // far exceeds it. A correct per-member bound passes; a cumulative bound (the
    // superseded whole-archive defect) would reject the archive.
    let member_count = 64usize;
    let lines: Vec<String> = (0..member_count)
        .map(|index| {
            let time_ms = 1_700_000_000_000 + index as i64 * 1_000;
            photo_line(INSTRUMENT_ONE.key, time_ms, "0.49", "0.51")
        })
        .collect();
    let owned: Vec<(String, Vec<u8>)> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| (format!("{index:03}.data"), line.as_bytes().to_vec()))
        .collect();
    let members_spec: Vec<(&str, &[u8])> = owned
        .iter()
        .map(|(name, data)| (name.as_str(), data.as_slice()))
        .collect();
    let archive = gzip_tar(&members_spec);

    let single_member_len = lines[0].len() as u64;
    // Bound generously above one member but below the aggregate member payload.
    let per_member_bound = single_member_len + 64;
    let decoded_payload_bytes: u64 = lines.iter().map(|line| line.len() as u64).sum();
    assert!(
        decoded_payload_bytes > per_member_bound,
        "the aggregate member payload must exceed the per-member bound"
    );

    let accepted = accepted_dataset();
    let decoded_bound = (member_count as u64 + 1) * 2 * TAR_BLOCK as u64;
    let limits = stream_limits(decoded_bound, per_member_bound);
    let tables = normalize_tar_jsonl_snapshot_deltas(
        &accepted,
        &keyed_identities(),
        &keyed_mapping(),
        &archive,
        &limits,
        42,
        "ingest-run-test",
    )
    .expect("per-member bound admits every small member");
    assert_eq!(tables.len(), 1, "all members carry the same instrument");
    // Each member contributes one photo = CLEAR + bid ADD + ask ADD = 3 rows.
    assert_eq!(tables[0].rows.len(), member_count * 3);
}

#[test]
fn tar_snapshot_adapter_enforces_the_shared_cumulative_decode_bound() {
    let member = photo_line(INSTRUMENT_ONE.key, 1_700_000_000_000, "0.49", "0.51");
    let archive = gzip_tar(&[("000.data", member.as_bytes())]);
    let limits = JsonlStreamLimits {
        max_decoded_bytes: TAR_BLOCK as u64,
        max_members: 4,
        max_member_bytes: 1 << 20,
        max_record_bytes: 1 << 20,
        max_records: 4,
        member_suffix: Some(".data".to_string()),
    };

    let error = normalize_tar_jsonl_snapshot_deltas(
        &accepted_dataset(),
        &keyed_identities(),
        &keyed_mapping(),
        &archive,
        &limits,
        42,
        "ingest-run-test",
    )
    .expect_err("tar snapshot decoding must enforce cumulative decoded bytes");

    let message = format!("{error:#}");
    assert!(message.contains("max_decoded_bytes"), "{message}");
}
