//! Class guard for source-universe object-manifest `table_family` integrity.
//!
//! An object manifest's `table_family` is the NT canonical table the source data
//! lands in (e.g. "trades"), NOT the raw source family (e.g. "native_trades").
//! A manifest that records the source family instead is fixture drift: it
//! diverges from the manifest's own source proof and from every sibling venue,
//! and — because the conversion queue overrides `table_family` from its own spec
//! — the bad value is silently masked downstream and never validated at runtime.
//! Pin the invariant at the manifest so the drift fails loud instead of lurking.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::canonical_trades::{
    BAR_TABLE_FAMILY, DELTAS_TABLE_FAMILY, INDEX_PRICES_TABLE_FAMILY, MARK_PRICES_TABLE_FAMILY,
    QUOTE_TABLE_FAMILY, TRADE_TABLE_FAMILY,
};
use serde_json::Value;

const OBJECT_MANIFEST_SCHEMA: &str = "backfill-source-universe-object-manifest.v1";

fn object_manifest_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests",
    )
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read object-manifest dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_json(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
}

#[test]
fn every_object_manifest_table_family_is_a_registered_nt_family() {
    // The registered NT canonical table families, sourced from the canonical
    // constants (single source of truth) rather than hardcoded literals.
    let registered: BTreeSet<&str> = [
        TRADE_TABLE_FAMILY,
        BAR_TABLE_FAMILY,
        DELTAS_TABLE_FAMILY,
        QUOTE_TABLE_FAMILY,
        INDEX_PRICES_TABLE_FAMILY,
        MARK_PRICES_TABLE_FAMILY,
    ]
    .into_iter()
    .collect();

    let mut files = Vec::new();
    collect_json(&object_manifest_root(), &mut files);

    let mut checked = 0usize;
    for path in &files {
        let value: Value = serde_json::from_slice(&fs::read(path).expect("read manifest"))
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        if value.get("schema_version").and_then(Value::as_str) != Some(OBJECT_MANIFEST_SCHEMA) {
            continue;
        }
        let table_family = value
            .get("table_family")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{} has no string table_family", path.display()));
        assert!(
            registered.contains(table_family),
            "{} declares table_family={table_family:?}, which is not a registered NT table \
             family {registered:?}; an object manifest must record the NT canonical table the \
             source converts to, not the raw source family",
            path.display(),
        );
        checked += 1;
    }

    // Non-vacuous: the committed binance/bybit/pmxt universes contribute well
    // over this many object manifests (top-level + per-category). A broken walk
    // that silently checks nothing must fail, not pass.
    assert!(
        checked >= 8,
        "expected to validate the committed object manifests, only checked {checked} (walk likely broken)"
    );
}
