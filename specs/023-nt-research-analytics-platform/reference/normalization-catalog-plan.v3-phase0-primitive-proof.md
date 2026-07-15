# v3 Phase-0 — load-bearing-primitive proof (ConditionalCatalogWriter)

> Historical evidence only. `normalization-catalog-plan.v3.md` is superseded
> and removed from live authority by
> `historical-data-acquisition-architecture.v1.md`. The transcript below is
> preserved as provenance and does not authorize implementation.

Empirical proof that the panel's "load-bearing wall" (plan v3 §4.3 ConditionalCatalogWriter) works against the **live bolt-v2 dependency pins** (object_store 0.13.2, arrow/parquet 58.3.0, sha2 0.10). Run as a throwaway probe; this file is the durable record. When the Phase-0 build starts properly, this code seeds the isolated catalog-projector crate (under epic #437 / #438).

## What it proves
- **PROOF 1 (R-5):** arrow→parquet encode + read-back roundtrip using ONLY public `arrow`/`parquet` — confirms NT's `write_batches_to_object_store` (parquet.rs:170, put baked in at :197) does NOT need to be reused; the ~20-line encode is replicable.
- **PROOF 2 (R-3):** logical-content digest over canonical-sorted arrow `RowConverter` rows is deterministic — the idempotency key is logical rows, not non-deterministic parquet bytes.
- **PROOF 3 (local primitive underlying the BLOCKER, B-1/F2):** 24 concurrent `put_opts(PutMode::Create)` on one path → exactly 1 wins, 23 `AlreadyExists`, stored bytes are the single winner's. `LocalFileSystem` Create == `std::fs::hard_link` (object_store local.rs:372, atomically exclusive) — this proves the `object_store` API-level create-only semantics (exactly one winner, losers observe `AlreadyExists`, no TOCTOU, no torn write). It does NOT exercise S3's `If-None-Match` path: v3 §4.3.5 states LocalFileSystem's Create semantics differ and excludes it from the concurrency-proof acceptance criterion. This run therefore does NOT discharge the §4.3.5 BLOCKER acceptance, does NOT produce a `no_overwrite_proof`, and leaves Phase-0 prerequisite 0.6 (conditional-put + copy-if-not-exists probe against the configured or a conformance store — MinIO/R2 with `ETagMatch`) open.
- **PROOF 4 (B-1, naming convention only):** canonical roots use NT-native `<start>_<end>.parquet`; content/transform-hash keying is staging-only (NT never reads staging). This is a string-shape assertion on two literals pinning the naming convention; it does not exercise NT's runtime filename parse (`parse_filename_timestamps` / `query_files`), which the Phase-0 0.W round-trip proof must cover.

## Verified run output
```
PROOF 1 PASS: arrow->parquet encode + read-back roundtrip (3 rows, 784 bytes; public arrow/parquet only, no NT-private API)
PROOF 2 PASS: logical-content digest deterministic over canonical sorted RowConverter image (keyed on logical rows, not parquet bytes)
PROOF 3 PASS (BLOCKER): 24 concurrent put_opts(PutMode::Create) -> exactly 1 wrote, 23 got AlreadyExists, stored bytes intact (LocalFileSystem hard_link == S3 If-None-Match contract)
PROOF 4 PASS: canonical roots use NT-native '<start>_<end>.parquet'; content/transform-hash keying is staging-only

ALL PROOFS PASSED — ConditionalCatalogWriter load-bearing primitive verified locally against the live pins.
```

> **Scope correction (external review, 2026-06-12).** The transcript above and the `src/main.rs`
> listing below are the verbatim record of the throwaway run and are preserved unedited. Two phrases
> in that record overstate scope and must be read as corrected here: "LocalFileSystem hard_link ==
> S3 If-None-Match contract" asserts only that the two backends present the same create-only
> API-level contract shape (`PutMode::Create` → exactly one winner + `AlreadyExists` for the rest);
> the S3 `If-None-Match` path itself is NOT exercised by a LocalFileSystem run (v3 §4.3.5, which
> excludes LocalFileSystem from the BLOCKER concurrency-proof acceptance). "Verified locally" means
> the local primitive only. Phase-0 gates that remain OPEN and are NOT discharged by this file:
> 0.6 (conditional-put + copy-if-not-exists probe against the configured or a conformance store),
> 0.E (encoder strategy locked and recorded in `NtCapabilityProof` — PROOF 1 is input evidence for
> that decision, not the decision), 0.W (`ConditionalCatalogWriter` build with the §4.3.5
> concurrency proof against a qualifying store, producing `no_overwrite_proof`), and 0.R
> (run-pinned-set / competing-promotion proof).

## Cargo.toml
```toml
[package]
name = "catalog-writer-proof"
version = "0.0.0"
edition = "2021"

[dependencies]
object_store = "0.13.2"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
bytes = "1"
sha2 = "0.10"
arrow = "58.3.0"
parquet = "58.3.0"
futures = "0.3"
anyhow = "1"
tempfile = "3"

[[bin]]
name = "proof"
path = "src/main.rs"
```

## src/main.rs
```rust
// Phase-0 load-bearing-primitive proof for the ConditionalCatalogWriter (plan v3 §4.3).
// Proves, against object_store 0.13.2 + arrow/parquet 58.3.0 (the live bolt-v2 pins):
//   1. arrow->parquet encode + read-back roundtrip using ONLY public arrow/parquet (R-5: no NT-private API).
//   2. logical-content digest over canonical-sorted arrow RowConverter rows is deterministic (R-3: key on logical rows, not parquet bytes).
//   3. THE BLOCKER: N concurrent put_opts(PutMode::Create) -> exactly one writer wins, rest get AlreadyExists,
//      stored bytes are the single winner's (LocalFileSystem Create == hard_link, atomically exclusive == S3 If-None-Match contract).
//   4. canonical roots use NT-native '<start>_<end>.parquet' names (B-1); content/transform-hash keying is staging-only.
use std::sync::Arc;
use anyhow::Result;
use bytes::Bytes;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload, path::Path as OsPath};
use object_store::local::LocalFileSystem;
use arrow::array::{ArrayRef, Int64Array, StringArray};
use arrow::record_batch::RecordBatch;
use arrow::row::{RowConverter, SortField};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use sha2::{Digest, Sha256};

fn sample_batch() -> RecordBatch {
    let ts = Int64Array::from(vec![1000i64, 2000, 3000]);
    let px = StringArray::from(vec!["100.5", "101.0", "99.75"]);
    RecordBatch::try_from_iter(vec![
        ("ts_event", Arc::new(ts) as ArrayRef),
        ("price", Arc::new(px) as ArrayRef),
    ])
    .unwrap()
}

fn reordered_batch() -> RecordBatch {
    let px = StringArray::from(vec!["99.75", "101.0", "100.5"]);
    let ts = Int64Array::from(vec![3000i64, 2000, 1000]);
    RecordBatch::try_from_iter(vec![
        ("price", Arc::new(px) as ArrayRef),
        ("ts_event", Arc::new(ts) as ArrayRef),
    ])
    .unwrap()
}

// Replicates NT's encode (parquet.rs:181-194): SNAPPY + max_row_group_row_count 5000, public arrow/parquet only.
fn encode_batch_to_parquet(batch: &RecordBatch) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_max_row_group_row_count(Some(5000))
        .build();
    let mut w = ArrowWriter::try_new(&mut buf, batch.schema(), Some(props))?;
    w.write(batch)?;
    w.close()?;
    Ok(buf)
}

// R-3: logical-content digest over the canonical schema image + sorted RowConverter row bytes (NOT parquet bytes).
fn logical_digest(batch: &RecordBatch) -> Result<[u8; 32]> {
    let mut columns: Vec<_> = batch
        .schema()
        .fields()
        .iter()
        .enumerate()
        .map(|(idx, f)| {
            (f.name().clone(), f.data_type().clone(), batch.column(idx).clone())
        })
        .collect();
    columns.sort_by(|a, b| a.0.cmp(&b.0));

    let fields: Vec<SortField> = columns
        .iter()
        .map(|(_, data_type, _)| SortField::new(data_type.clone()))
        .collect();
    let sorted_columns: Vec<ArrayRef> = columns
        .iter()
        .map(|(_, _, column)| column.clone())
        .collect();
    let converter = RowConverter::new(fields)?;
    let rows = converter.convert_columns(&sorted_columns)?;
    let mut row_images: Vec<Vec<u8>> = rows.iter().map(|row| row.as_ref().to_vec()).collect();
    row_images.sort();

    let mut hasher = Sha256::new();
    for (name, data_type, _) in columns.iter() {
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        let data_type_image = format!("{data_type:?}");
        hasher.update((data_type_image.len() as u64).to_le_bytes());
        hasher.update(data_type_image.as_bytes());
    }
    for row in row_images {
        hasher.update((row.len() as u64).to_le_bytes());
        hasher.update(row);
    }
    Ok(hasher.finalize().into())
}

async fn put_create(
    store: &dyn ObjectStore,
    path: &OsPath,
    bytes: Vec<u8>,
) -> std::result::Result<bool, object_store::Error> {
    match store
        .put_opts(
            path,
            PutPayload::from(bytes),
            PutOptions { mode: PutMode::Create, ..Default::default() },
        )
        .await
    {
        Ok(_) => Ok(true),
        Err(object_store::Error::AlreadyExists { .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<()> {
    // PROOF 1 — encode + read-back roundtrip (R-5)
    let batch = sample_batch();
    let pq = encode_batch_to_parquet(&batch)?;
    assert!(!pq.is_empty());
    let reader = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(pq.clone()))?.build()?;
    let mut rows_back = 0usize;
    for rb in reader {
        rows_back += rb?.num_rows();
    }
    assert_eq!(rows_back, batch.num_rows(), "roundtrip row count");
    println!(
        "PROOF 1 PASS: arrow->parquet encode + read-back roundtrip ({} rows, {} bytes; public arrow/parquet only, no NT-private API)",
        rows_back,
        pq.len()
    );

    // PROOF 2 — logical digest deterministic (R-3)
    let d1 = logical_digest(&batch)?;
    let d2 = logical_digest(&sample_batch())?;
    let d3 = logical_digest(&reordered_batch())?;
    assert_eq!(d1, d2, "logical digest stable for identical logical rows");
    assert_eq!(
        d1, d3,
        "logical digest stable across column ordering and row ordering"
    );
    println!(
        "PROOF 2 PASS: logical-content digest deterministic over canonical sorted RowConverter image (keyed on logical rows, not parquet bytes)"
    );

    // PROOF 3 — THE BLOCKER: N-writer conditional-create race
    let dir = tempfile::tempdir()?;
    let store: Arc<dyn ObjectStore> = Arc::new(LocalFileSystem::new_with_prefix(dir.path())?);
    let path = OsPath::from(
        "nt-catalog/sets/SET1/data/quotes/INSTR-1/0000000000001000_0000000000003000.parquet",
    );
    let n = 24usize;
    let mut handles = Vec::new();
    for _ in 0..n {
        let s = store.clone();
        let p = path.clone();
        let payload = pq.clone();
        handles.push(tokio::spawn(async move { put_create(s.as_ref(), &p, payload).await }));
    }
    let (mut wrote, mut existed) = (0usize, 0usize);
    for h in handles {
        match h.await.unwrap()? {
            true => wrote += 1,
            false => existed += 1,
        }
    }
    assert_eq!(wrote, 1, "exactly one writer wins the create race (got {wrote})");
    assert_eq!(existed, n - 1, "all other writers observe AlreadyExists (got {existed})");
    let got = store.get(&path).await?.bytes().await?;
    assert_eq!(got.as_ref(), pq.as_slice(), "stored bytes are the single winner's, not torn/overwritten");
    println!(
        "PROOF 3 PASS (BLOCKER): {n} concurrent put_opts(PutMode::Create) -> exactly 1 wrote, {existed} got AlreadyExists, stored bytes intact (LocalFileSystem hard_link == S3 If-None-Match contract)"
    );

    // PROOF 4 — canonical NT-native filenames vs staging hash-keyed (B-1)
    let canonical = "0000000000001000_0000000000003000.parquet";
    let staging = "0000000000001000_0000000000003000__t-deadbeef__c-cafef00d.parquet";
    let canon_stem = canonical.strip_suffix(".parquet").unwrap();
    assert_eq!(canon_stem.split('_').count(), 2, "canonical splits into exactly 2 interval parts (NT parse_filename_timestamps split_once('_'))");
    assert!(!canonical.contains("__t-") && !canonical.contains("__c-"), "canonical carries no staging hash suffix");
    assert!(staging.contains("__t-") && staging.contains("__c-"), "staging name carries content/transform hash (NT never reads staging)");
    println!("PROOF 4 PASS: canonical roots use NT-native '<start>_<end>.parquet'; content/transform-hash keying is staging-only");

    println!("\nALL PROOFS PASSED — ConditionalCatalogWriter load-bearing primitive verified locally against the live pins.");
    Ok(())
}
```
