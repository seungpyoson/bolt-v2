use std::{fs, path::Path};

use crate::backtesting_vertical_slice_test_support::{
    assert_generated_fixture_matches_index, generate_evicted_pmxt_object_manifests,
    materialize_evicted_pmxt_object_manifests, tempdir_in_repo_target,
};
use backtesting_vertical_slice::{
    source_proof::{
        CheckOutcome, EvidenceState, SourceProofFidelityClass, SourceProofReport, SourceProofStatus,
    },
    source_universe_source_proofs::{
        SourceUniverseSourceProofSet, write_source_universe_source_proof_set_from_spec_file,
    },
};

#[test]
fn binance_all_instrument_category_source_proofs_are_materialized_from_manifests() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("source-proofs");
    let spec_path = temp_dir.path().join("source-universe-source-proofs.toml");
    let manifest_root = reference_root
        .join("backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/category-manifests");

    fs::write(
        &spec_path,
        format!(
            r#"
proof_set_id = "source-universe-source-proofs-binance-data-vision-trades-2026-03-01-all-instruments"
output_dir = "{output_dir}"
source_bindings_path = "{source_bindings_path}"
venue = "binance"
table_family = "trades"
manifest_table_family = "native_trades"
source_candidate_class = "official_free"
source_selection_status = "ACCEPTED_LOWER_FIDELITY"
usage_scope = "canonical_backfill_input"
fidelity_class = "TRADE_REPLAY"
acceptance_mode = "manual"
accepted_by = "backtesting-vertical-slice-operator"
accepted_at_utc = "2026-06-10T00:00:00Z"
requested_start_utc = "2026-03-01T00:00:00Z"
requested_end_utc = "2026-03-02T00:00:00Z"
coverage_start_utc = "2026-03-01T00:00:00Z"
coverage_end_utc = "2026-03-02T00:00:00Z"
license_ref = "https://github.com/binance/binance-public-data README: Licence MIT; observed 2026-06-07"
license_scope = "public"
retention_ref = "https://github.com/binance/binance-public-data README: daily/monthly public archive and checksum sidecars; observed 2026-06-07"
cost_ref = "cost://free-public-archive"
gap_policy_id = ""
raw_sample_selection = "first_manifest_record"
schema_sample_policy = "raw_sample"

[required_checks]
source_access = "Binance Data Vision source URLs are enumerated in category manifest {{manifest_id}}"
license = "Binance public data README license review permits BTE canonical/backtest input"
schema = "Schema columns are committed in category manifest {{manifest_id}}"
time_semantics = "Binance native trade time column maps to Unix milliseconds"
instrument_universe = "{{instrument_universe_id}} category={{category}} instrument_count={{instrument_count}}"
coverage = "category={{category}} object_count={{object_count}} archive_date_range=[{{first_archive_date}},{{last_archive_date}}]"
retention_freshness = "Binance public data archive retention reviewed 2026-06-07"
granularity = "Binance Data Vision daily trades files are native trade prints"
completeness = "object_count={{object_count}} compressed_bytes={{accepted_bytes}} from committed category manifest"
nt_mapping = "nautilus_model::data::TradeTick; converter mappings are committed for Binance native trade CSV contracts"
cost = "cost://free-public-archive"
storage = "raw objects are staged under category manifest {{manifest_id}}"

[[claim_limit]]
id = "source-proof-claim-limit-001"
severity = "blocking"
claim = "No execution-quality, queue-position, or order-book-liquidity claims."
reason = "TRADE_REPLAY source proof is native trade-print replay, not order-book replay."
evidence_ref = "source-proof://{{source_proof_id}}/fidelity"

[[claim_limit]]
id = "source-proof-claim-limit-002"
severity = "blocking"
claim = "No L2/L3 order-book replay claims from trade prints."
reason = "The accepted category manifest contains trade prints and no L2/L3 order-book deltas or depth snapshots."
evidence_ref = "source-proof://{{source_proof_id}}/schema"

[[claim_limit]]
id = "source-proof-claim-limit-003"
severity = "blocking"
claim = "No historical venue-rule, fillability, rounding, sizing, or execution-quality claims."
reason = "The category proof establishes native trade-print replay input only; dated venue-rule and execution-admissibility evidence remains outside this proof."
evidence_ref = "source-proof://{{source_proof_id}}/claim-limits"

[[source_binding]]
source_binding = "binance-spot-native-trades"
source_proof_id = "source-proof-binance-spot-native-trades-2026-03-01-all-instruments"
product_category = "spot"
instrument_universe_id = "binance-spot-instruments-2026-03-01-all-instruments"
category_manifest_path = "{manifest_root}/binance-data-vision-trades-object-manifest-spot.json"

[[source_binding]]
source_binding = "binance-usd-m-perpetual-native-trades"
source_proof_id = "source-proof-binance-usd-m-perpetual-native-trades-2026-03-01-all-instruments"
product_category = "usd_m_perpetual"
instrument_universe_id = "binance-usd-m-perpetual-instruments-2026-03-01-all-instruments"
category_manifest_path = "{manifest_root}/binance-data-vision-trades-object-manifest-usd_m_perpetual.json"

[[source_binding]]
source_binding = "binance-usd-m-delivery-native-trades"
source_proof_id = "source-proof-binance-usd-m-delivery-native-trades-2026-03-01-all-instruments"
product_category = "usd_m_delivery"
instrument_universe_id = "binance-usd-m-delivery-instruments-2026-03-01-all-instruments"
category_manifest_path = "{manifest_root}/binance-data-vision-trades-object-manifest-usd_m_delivery.json"

[[source_binding]]
source_binding = "binance-coin-m-perpetual-native-trades"
source_proof_id = "source-proof-binance-coin-m-perpetual-native-trades-2026-03-01-all-instruments"
product_category = "coin_m_perpetual"
instrument_universe_id = "binance-coin-m-perpetual-instruments-2026-03-01-all-instruments"
category_manifest_path = "{manifest_root}/binance-data-vision-trades-object-manifest-coin_m_perpetual.json"

[[source_binding]]
source_binding = "binance-coin-m-delivery-native-trades"
source_proof_id = "source-proof-binance-coin-m-delivery-native-trades-2026-03-01-all-instruments"
product_category = "coin_m_delivery"
instrument_universe_id = "binance-coin-m-delivery-instruments-2026-03-01-all-instruments"
category_manifest_path = "{manifest_root}/binance-data-vision-trades-object-manifest-coin_m_delivery.json"
"#,
            output_dir = output_dir.display(),
            source_bindings_path = reference_root
                .join("backfill-source-bindings.v1.toml")
                .display(),
            manifest_root = manifest_root.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_source_proof_set_from_spec_file(&spec_path)
        .expect("source proof set writes");
    let proof_set: SourceUniverseSourceProofSet =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read proof set"))
            .expect("parse proof set");

    assert_eq!(proof_set.proof_count, 5);
    assert_eq!(proof_set.total_completed_objects, 2_051);
    assert_eq!(proof_set.total_accepted_bytes, 1_748_721_970);
    assert_eq!(proof_set.accepted_proof_count, 5);

    let spot_path =
        output_dir.join("source-proof-binance-spot-native-trades-2026-03-01-all-instruments.json");
    let spot: SourceProofReport =
        serde_json::from_slice(&fs::read(spot_path).expect("read spot proof"))
            .expect("parse spot proof");
    assert_eq!(spot.status, SourceProofStatus::Accepted);
    assert_eq!(spot.source_binding, "binance-spot-native-trades");
    assert_eq!(spot.product_family, "spot");
    assert_eq!(spot.product_category, "spot");
    assert_eq!(
        spot.acceptance_scope.as_ref().unwrap().completed_objects,
        1_416
    );
    assert_eq!(
        spot.raw_sample_hash,
        "054fa3d832a2e262855e93f46f636e66b1bac17b29caf4cd6e4a597b494422e4"
    );
    spot.evaluate_acceptance()
        .expect("generated spot proof is accepted by source-proof gate");
}

#[test]
fn source_universe_source_proofs_preserve_configured_l2_replay_evidence() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("source-proofs");
    let spec_path = temp_dir.path().join("source-universe-source-proofs.toml");
    let manifest_path = reference_root.join(
        "backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/category-manifests/binance-data-vision-trades-object-manifest-spot.json",
    );

    fs::write(
        &spec_path,
        format!(
            r#"
proof_set_id = "source-universe-source-proofs-binance-l2-evidence-regression"
output_dir = "{output_dir}"
source_bindings_path = "{source_bindings_path}"
venue = "binance"
table_family = "trades"
manifest_table_family = "native_trades"
source_candidate_class = "official_free"
source_selection_status = "ACCEPTED_FOR_REQUIRED_FIDELITY"
usage_scope = "canonical_backfill_input"
fidelity_class = "L2_REPLAY"
acceptance_mode = "manual"
accepted_by = "backtesting-vertical-slice-operator"
accepted_at_utc = "2026-06-10T00:00:00Z"
requested_start_utc = "2026-03-01T00:00:00Z"
requested_end_utc = "2026-03-02T00:00:00Z"
coverage_start_utc = "2026-03-01T00:00:00Z"
coverage_end_utc = "2026-03-02T00:00:00Z"
license_ref = "https://github.com/binance/binance-public-data README: Licence MIT; observed 2026-06-07"
license_scope = "public"
retention_ref = "https://github.com/binance/binance-public-data README: daily/monthly public archive and checksum sidecars; observed 2026-06-07"
cost_ref = "cost://free-public-archive"
gap_policy_id = ""
raw_sample_selection = "first_manifest_record"
schema_sample_policy = "raw_sample"

[l2_replay_evidence]
order_book_delta_ref = "source-proof://source-universe-source-proofs-binance-l2-evidence-regression/order-book-delta"
no_tick_size_change_universe_ref = "source-proof://source-universe-source-proofs-binance-l2-evidence-regression/no-tick-size-change"

[required_checks]
source_access = "Binance Data Vision source URLs are enumerated in category manifest {{manifest_id}}"
license = "Binance public data README license review permits BTE canonical/backtest input"
schema = "Schema columns are committed in category manifest {{manifest_id}}"
time_semantics = "Binance native trade time column maps to Unix milliseconds"
instrument_universe = "{{instrument_universe_id}} category={{category}} instrument_count={{instrument_count}}"
coverage = "category={{category}} object_count={{object_count}} archive_date_range=[{{first_archive_date}},{{last_archive_date}}]"
retention_freshness = "Binance public data archive retention reviewed 2026-06-07"
granularity = "Synthetic L2 replay evidence regression binds configured evidence fields"
completeness = "object_count={{object_count}} compressed_bytes={{accepted_bytes}} from committed category manifest"
nt_mapping = "Configured L2 replay evidence is carried into SourceProofReport"
cost = "cost://free-public-archive"
storage = "raw objects are staged under category manifest {{manifest_id}}"

[[claim_limit]]
id = "source-proof-claim-limit-l2-evidence-regression"
severity = "blocking"
claim = "No dynamic tick-size replay claim outside the configured tick-size policy evidence."
reason = "The generated source proof must bind explicit L2 replay and tick-size policy evidence."
evidence_ref = "source-proof://{{source_proof_id}}/l2-replay-evidence"

[[source_binding]]
source_binding = "binance-spot-native-trades"
source_proof_id = "source-proof-binance-l2-evidence-regression"
product_category = "spot"
instrument_universe_id = "binance-spot-instruments-2026-03-01-all-instruments"
category_manifest_path = "{manifest_path}"
"#,
            output_dir = output_dir.display(),
            source_bindings_path = reference_root
                .join("backfill-source-bindings.v1.toml")
                .display(),
            manifest_path = manifest_path.display(),
        ),
    )
    .expect("write spec");

    write_source_universe_source_proof_set_from_spec_file(&spec_path)
        .expect("source proof set writes");
    let proof_path = output_dir.join("source-proof-binance-l2-evidence-regression.json");
    let proof: SourceProofReport =
        serde_json::from_slice(&fs::read(proof_path).expect("read generated proof"))
            .expect("parse generated proof");

    assert_eq!(proof.fidelity_class, SourceProofFidelityClass::L2Replay);
    assert_eq!(
        proof.l2_replay_evidence.order_book_delta_ref.as_deref(),
        Some(
            "source-proof://source-universe-source-proofs-binance-l2-evidence-regression/order-book-delta"
        )
    );
    assert_eq!(
        proof
            .l2_replay_evidence
            .no_tick_size_change_universe_ref
            .as_deref(),
        Some(
            "source-proof://source-universe-source-proofs-binance-l2-evidence-regression/no-tick-size-change"
        )
    );
    proof
        .evaluate_acceptance()
        .expect("configured L2 evidence produces accepted source proof");
}

#[test]
fn source_universe_source_proofs_materialize_pmxt_pending_manifest_scoped_l2_proof() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempdir_in_repo_target();
    let output_dir = temp_dir.path().join("source-proofs");
    let spec_path = temp_dir.path().join("source-universe-source-proofs.toml");
    let (_, manifest_path) =
        generate_evicted_pmxt_object_manifests(&reference_root, temp_dir.path());

    fs::write(
        &spec_path,
        format!(
            r#"
proof_set_id = "source-universe-source-proofs-pmxt-polymarket-v2-current"
output_dir = "{output_dir}"
source_bindings_path = "{source_bindings_path}"
venue = "polymarket"
table_family = "order_book_snapshot_deltas"
manifest_table_family = "order_book_snapshot_deltas"
status = "pending"
source_candidate_class = "official_free"
source_selection_status = "PENDING_MORE_PROOF"
usage_scope = "one_off_backfill_data"
fidelity_class = "L2_REPLAY"
requested_start_utc = "2026-04-13T19:00:00Z"
requested_end_utc = "2026-06-10T16:00:00Z"
coverage_start_utc = "2026-04-13T19:00:00Z"
coverage_end_utc = "2026-06-10T16:00:00Z"
license_ref = "https://archive.pmxt.dev/docs/v2-data-overview#license"
license_scope = "public"
retention_ref = "pending://source-proofs/pmxt-polymarket-v2-current/retention-freshness"
cost_ref = "pending://source-proofs/pmxt-polymarket-v2-current/cost"
gap_policy_id = ""
raw_sample_selection = "first_manifest_record"
schema_sample_policy = "raw_sample"

[l2_replay_evidence]
order_book_delta_ref = "repo://specs/023-nt-research-analytics-platform/reference/source-proof-nt-mapping-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json"
sufficient_snapshot_cadence_ref = "repo://specs/023-nt-research-analytics-platform/reference/source-proof-sample-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json"

[required_checks.source_access]
outcome = "passed"
evidence_ref = "PMXT archive objects are enumerated in category manifest {{manifest_id}}"

[required_checks.license]
outcome = "passed"
evidence_ref = "PMXT archive license review permits public research use"

[required_checks.schema]
outcome = "passed"
evidence_ref = "Schema columns are committed in category manifest {{manifest_id}}"

[required_checks.time_semantics]
outcome = "passed"
evidence_ref = "timestamp_received is the archive-hour axis; timestamp is upstream event time"

[required_checks.instrument_universe]
outcome = "pending"
evidence_ref = "pending broad BinaryOption instrument-universe proof for {{instrument_universe_id}}"

[required_checks.coverage]
outcome = "pending"
evidence_ref = "pending coverage proof for object_count={{object_count}} archive_date_range=[{{first_archive_date}},{{last_archive_date}}]"

[required_checks.retention_freshness]
outcome = "pending"
evidence_ref = "pending durable archive retention and update-lag proof"

[required_checks.granularity]
outcome = "passed"
evidence_ref = "PMXT v2 orderbook files include book, price_change, last_trade_price, and tick_size_change events"

[required_checks.completeness]
outcome = "pending"
evidence_ref = "pending completeness proof for compressed_bytes={{accepted_bytes}}"

[required_checks.nt_mapping]
outcome = "passed"
evidence_ref = "bounded PMXT selected-source NT mapping evidence exists for OrderBookDelta and TradeTick"

[required_checks.cost]
outcome = "pending"
evidence_ref = "pending accepted cost proof for 557815904970 bytes of PMXT source archive data"

[required_checks.storage]
outcome = "pending"
evidence_ref = "pending artifact-root staging proof for PMXT source-proof evidence"

[[claim_limit]]
id = "pmxt-source-proof-claim-limit-001"
severity = "blocking"
claim = "No canonical, production, or broad NT catalog/backtest input from this pending PMXT L2 source proof."
reason = "The generated proof is manifest-scoped but remains pending until coverage, cost, storage, completeness, and tick-size policy evidence are accepted."
evidence_ref = "source-proof://{{source_proof_id}}/status"

[[claim_limit]]
id = "pmxt-source-proof-claim-limit-002"
severity = "blocking"
claim = "No dynamic tick-size replay claim until NT-native timed instrument-epoch replay or a source-proof-bound no-tick-size-change universe is accepted."
reason = "The PMXT source includes tick_size_change fields and the current broad replay policy remains unaccepted."
evidence_ref = "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-polymarket-tick-size-change-status.2026-06-08.json"

[[source_binding]]
source_binding = "polymarket-parquet-archive-index"
source_proof_id = "source-proof-pmxt-polymarket-v2-current-orderbook"
product_category = "binary-option"
instrument_universe_id = "pmxt-polymarket-v2-current-orderbook"
category_manifest_path = "{manifest_path}"
"#,
            output_dir = output_dir.display(),
            source_bindings_path = reference_root
                .join("backfill-source-bindings.v1.toml")
                .display(),
            manifest_path = manifest_path.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_source_proof_set_from_spec_file(&spec_path)
        .expect("pending source proof set writes");
    let proof_set: SourceUniverseSourceProofSet =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read proof set"))
            .expect("parse proof set");
    assert_eq!(proof_set.proof_count, 1);
    assert_eq!(proof_set.accepted_proof_count, 0);
    assert_eq!(proof_set.total_completed_objects, 1_351);
    assert_eq!(proof_set.total_accepted_bytes, 557_815_904_970);

    let proof_path = output_dir.join("source-proof-pmxt-polymarket-v2-current-orderbook.json");
    let proof: SourceProofReport =
        serde_json::from_slice(&fs::read(proof_path).expect("read generated proof"))
            .expect("parse generated proof");

    assert_eq!(proof.status, SourceProofStatus::Pending);
    assert_eq!(proof.fidelity_class, SourceProofFidelityClass::L2Replay);
    assert_eq!(proof.evidence_state, EvidenceState::PendingSourceProof);
    assert!(proof.acceptance_mode.is_none());
    assert!(proof.accepted_by.is_none());
    assert!(proof.accepted_at.is_none());
    assert_eq!(
        proof.required_checks.coverage.outcome,
        CheckOutcome::Pending
    );
    assert_eq!(proof.required_checks.storage.outcome, CheckOutcome::Pending);
    assert_eq!(
        proof
            .acceptance_scope
            .as_ref()
            .expect("acceptance scope")
            .completed_objects,
        1_351
    );
    assert!(
        proof
            .evaluate_acceptance()
            .expect_err("pending PMXT proof must remain non-accepted")
            .to_string()
            .contains("evidence_state")
    );
}

#[test]
fn committed_pmxt_source_proof_spec_regenerates_evicted_indexed_outputs() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    materialize_evicted_pmxt_object_manifests(&reference_root);
    let spec_path = reference_root.join(
        "backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
    );
    let artifact = write_source_universe_source_proof_set_from_spec_file(&spec_path)
        .expect("committed PMXT source-proof spec regenerates into scratch");
    let proof_path = artifact
        .path
        .parent()
        .expect("proof-set output parent")
        .join("source-proof-pmxt-polymarket-v2-current-orderbook.json");
    assert_generated_fixture_matches_index(
        "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proof-set.json",
        &artifact.path,
    );
    assert_generated_fixture_matches_index(
        "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-proof-pmxt-polymarket-v2-current-orderbook.json",
        &proof_path,
    );
}
