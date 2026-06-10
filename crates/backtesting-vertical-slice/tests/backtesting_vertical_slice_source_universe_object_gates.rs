use std::{fs, path::Path};

use backtesting_vertical_slice::source_universe_object_gates::{
    SourceUniverseObjectGateMaterialization, SourceUniverseObjectGateStatus,
    write_source_universe_object_gate_materialization_from_spec_file,
};

#[test]
fn source_universe_object_gates_cover_every_bybit_queue_item() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("object-gates");
    let spec_path = temp_dir.path().join("source-universe-object-gates.toml");

    fs::write(
        &spec_path,
        format!(
            r#"
gate_id = "source-universe-object-gates-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
queue_path = "{queue_path}"
output_dir = "{output_dir}"

[[source_binding]]
source_binding = "bybit-spot-tick-trades"
source_proof_path = "{spot_proof}"
category_manifest_path = "{spot_manifest}"

[[source_binding]]
source_binding = "bybit-linear-tick-trades"
source_proof_path = "{linear_proof}"
category_manifest_path = "{linear_manifest}"

[[source_binding]]
source_binding = "bybit-inverse-tick-trades"
source_proof_path = "{inverse_proof}"
category_manifest_path = "{inverse_manifest}"
"#,
            output_dir = output_dir.display(),
            queue_path = reference_root
                .join("source-universe-conversion-queues/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/queue/source-universe-conversion-queue.json")
                .display(),
            spot_proof = reference_root
                .join("backfill-source-proofs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-proof-bybit-spot-public-archive-tick-trades.json")
                .display(),
            spot_manifest = reference_root
                .join("backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/category-manifests/bybit-public-archive-tick-trades-object-manifest-spot.json")
                .display(),
            linear_proof = reference_root
                .join("backfill-source-proofs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-proof-bybit-linear-public-archive-tick-trades.json")
                .display(),
            linear_manifest = reference_root
                .join("backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/category-manifests/bybit-public-archive-tick-trades-object-manifest-linear.json")
                .display(),
            inverse_proof = reference_root
                .join("backfill-source-proofs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-proof-bybit-inverse-public-archive-tick-trades.json")
                .display(),
            inverse_manifest = reference_root
                .join("backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/category-manifests/bybit-public-archive-tick-trades-object-manifest-inverse.json")
                .display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_object_gate_materialization_from_spec_file(&spec_path)
        .expect("object gates generate");
    let gates: SourceUniverseObjectGateMaterialization =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read gates"))
            .expect("parse gates");

    assert_eq!(gates.status, SourceUniverseObjectGateStatus::Ready);
    assert_eq!(
        gates.queue_id,
        "source-universe-conversion-queue-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
    );
    assert_eq!(gates.work_item_count, 5_857);
    assert_eq!(gates.accepted_gate_count, 5_857);
    assert_eq!(gates.total_accepted_bytes, 20_309_079_098);
    assert_eq!(gates.source_binding_count, 3);
    assert_eq!(gates.source_binding_summaries.len(), 3);

    let inverse = gates
        .source_binding_summaries
        .iter()
        .find(|summary| summary.source_binding == "bybit-inverse-tick-trades")
        .expect("inverse summary");
    assert_eq!(inverse.work_item_count, 702);
    assert_eq!(inverse.accepted_bytes, 624_992_483);
    assert_eq!(
        inverse.source_proof_id,
        "source-proof-bybit-inverse-public-archive-tick-trades-2025-06-01-2026-06-01"
    );

    let first = gates.records.first().expect("first gate record");
    assert_eq!(first.source_binding, "bybit-inverse-tick-trades");
    assert_eq!(first.category, "inverse");
    assert_eq!(first.symbol, "AAVEUSD");
    assert_eq!(first.archive_date, "2025-06-01");
    assert_eq!(
        first.selected_object_sha256,
        "0c92b646ffca8f0621eb36741b3d7382c9212905d781905ff066bfc0b5d72516"
    );
    assert_eq!(first.selected_object_bytes, 124_717);
    assert!(
        first
            .source_proof_scope_report_id
            .contains(first.work_item_id.as_str())
    );
    assert!(
        first
            .accepted_tranche_id
            .contains(first.work_item_id.as_str())
    );
}
