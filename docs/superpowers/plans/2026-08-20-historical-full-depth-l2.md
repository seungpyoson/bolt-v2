# Historical Full-Depth L2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert native OKX and Bybit seeded order-book archives into replayable full-depth L2 in the existing NautilusTrader catalog path while preserving the issue-789 quote behavior and keeping memory/storage bounded.

**Architecture:** The only authority path remains `AcceptedDataset -> CanonicalOrderBookDeltasTable -> project_canonical_order_book_deltas_to_catalog -> NT catalog`. A config-driven seeded level-set converter scans all four supported archive containers in encounter order, maintains one capped NT L2 book, and emits only a TOML-selected event window plus one replay seed; BBO is derived once per selected source event from the NT book. After differential proof, the old raw-to-BBO `SeededL2Quotes` path is deleted.

**Tech Stack:** Rust, serde/TOML, flate2, tar, zip, NautilusTrader `OrderBookDelta`/`OrderBookDeltas`/`OrderBook`/`ParquetDataCatalog`, Arrow/Parquet, cargo test.

**Spec:** GitHub issue #789 and the externally reviewed design constraints captured under “Global Constraints” below.

## Global Constraints

- Work only in `/Users/spson/worktrees/bolt-v2/codex/historical-full-depth-l2`; the final review branch is `codex/historical-full-depth-l2-pr`, based on exact main `ffe03465b4c654f9d228e8f9cdc9397010d42135`.
- Keep one data path: accepted evidence to canonical full-depth L2 deltas to the existing NT catalog bridge. Do not create a raw-to-NT bypass, alternate catalog, sidecar market-data model, or second BBO implementation.
- Select behavior from TOML. Converter code contains no OKX/Bybit branches or hardcoded instrument IDs, time windows, tuple shapes, limits, sequence policy, or action values.
- Support `JsonlText`, `JsonlGzip`, `SingleJsonlZip`, and `TarGzipJsonl` through one bounded record visitor.
- Bound compressed object bytes, cumulative decoded bytes, archive members, member bytes, JSON record bytes/count, levels per event, active levels per side, selected source events, selected delta rows, and emitted bytes. Exceeding any bound fails closed.
- Preserve archive encounter order. Never sort seeded incremental events.
- Snapshot source events emit `CLEAR` followed by `ADD`; positive incremental levels emit `UPDATE`; zero-size levels emit `DELETE`. All L2 rows carry `F_MBP`, snapshot rows also carry `F_SNAPSHOT`, and only the final row of each source event carries `F_LAST`.
- Set canonical `availability_time` to the source event time on every emitted row. Catalog projection uses that value for `ts_init`, preventing the issue-789 batch-timestamp freeze.
- `CanonicalOrderBookDeltaRow.sequence` is a dense audit row ordinal. NT `OrderBookDelta.sequence` uses the numeric native source-event sequence for every row of that event, or `0` when the venue provides no native sequence.
- Validate configured order-count fields as nonnegative integers, but declare and test their intentional loss because NT full-depth `OrderBookDelta` cannot represent per-level order counts. Do not encode counts into `order_id` and do not emit `OrderBookDepth10`.
- Maintain a capped NT L2 book while scanning. At the selected window boundary, emit one reconstructed snapshot seed representing the immediately preceding state; the seed establishes replay state and does not emit a quote.
- Emit one derived quote after each selected source event only when both sides exist, matching the accepted issue-789 cardinality. Group source events by `F_LAST`; do not use NT `deltas_to_quotes`.
- No L3 claims, on-chain work, provider purchasing, credential work, or token-screener changes belong in this slice.
- The final review head must contain the new path and deletion of `SeededL2Quotes`; retaining both paths is not reviewable.

---

### Task 1: Correct canonical event and sequence semantics

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/canonical_market_data.rs`
- Modify: `crates/backtesting-vertical-slice/src/catalog_projection.rs`
- Modify: `crates/backtesting-vertical-slice/src/canonical_order_book_deltas.rs`
- Test: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_order_book_delta_projection.rs`

**Interfaces:**
- Consumes: `CanonicalOrderBookDeltasTable`, `CanonicalOrderBookDeltaRow.source_sequence`, and `availability_time`.
- Produces: `native_order_book_sequence(&CanonicalOrderBookDeltaRow) -> Result<u64>` and event-group validation where `F_LAST` closes one source event.

- [x] **Step 1: Write failing projection tests**

Add behavioral tests that project canonical rows into a temporary NT catalog and assert:

```rust
assert!(read_back.iter().all(|delta| delta.sequence == 0));
assert!(native_event_rows.iter().all(|delta| delta.sequence == 77));
assert!(project(non_numeric_source_sequence).is_err());
```

The native-sequence case must use multiple rows in one `F_LAST`-closed event. Add a same-`ts_init` snapshot test proving catalog read-back retains `CLEAR`, level rows, and the terminal `F_LAST` in exact event order.

- [x] **Step 2: Run the focused tests and observe RED**

Run:

```bash
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_order_book_delta_projection -- --nocapture
```

Expected: the absent-native-sequence case reports dense canonical row ordinals instead of `0`, and malformed native sequence does not fail.

- [x] **Step 3: Implement the minimal projection and validation changes**

Parse `source_sequence` as `u64` when present and project it into NT; use `0` when absent. Keep the dense canonical ordinal only in `CanonicalOrderBookDeltaRow.sequence`, and correct its documentation so it does not claim venue ownership. Validate rows as `F_LAST`-closed source-event groups with consistent instrument, event time, availability time, and native sequence; require `F_MBP` on L2 payload rows. Update existing event-stream expansion to emit `F_MBP` and one terminal `F_LAST` per source event.

- [x] **Step 4: Run focused and module tests to GREEN**

Run:

```bash
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_order_book_delta_projection -- --nocapture
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --lib canonical_order_book_deltas::tests -- --nocapture
```

Expected: all selected tests pass; exact catalog read-back order is proven at the pinned NT revision. If equal-timestamp order is not stable, stop this slice rather than synthesize a sequence.

- [x] **Step 5: Commit the verified semantic correction**

```bash
git add crates/backtesting-vertical-slice/src/canonical_market_data.rs crates/backtesting-vertical-slice/src/catalog_projection.rs crates/backtesting-vertical-slice/src/canonical_order_book_deltas.rs crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_order_book_delta_projection.rs
git commit -m "fix(backtest): preserve native L2 event semantics"
```

### Task 2: Add one bounded JSONL record visitor for every container

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/tar_reader.rs`
- Create: `crates/backtesting-vertical-slice/src/jsonl_record_stream.rs`
- Modify: `crates/backtesting-vertical-slice/src/lib.rs`
- Test: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_jsonl_record_stream.rs`
- Modify: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs`

**Interfaces:**
- Consumes: accepted raw bytes, `RawPayloadContainer`, and TOML-derived `JsonlStreamLimits`.
- Produces:

```rust
pub struct JsonlStreamLimits {
    pub max_decoded_bytes: u64,
    pub max_members: u64,
    pub max_member_bytes: u64,
    pub max_record_bytes: usize,
    pub max_records: u64,
    pub member_suffix: Option<String>,
}

pub struct JsonlScanStats {
    pub decoded_bytes: u64,
    pub members: u64,
    pub records: u64,
    pub peak_record_buffer_bytes: usize,
}

pub fn visit_jsonl_records(
    container: RawPayloadContainer,
    bytes: &[u8],
    limits: &JsonlStreamLimits,
    visit: impl FnMut(u64, &[u8]) -> Result<()>,
) -> Result<JsonlScanStats>;
```

- [x] **Step 1: Write failing behavioral tests**

Create fixtures in memory for plain JSONL, gzip JSONL, one-file ZIP, and multi-member tar.gz. Assert all containers yield the same ordered records and stats. Add fail-closed tests for record length, cumulative decoded bytes, member count/bytes, record count, truncated compression, invalid UTF-8, and ZIPs with zero or multiple JSONL members. Use a generated large single-member tar and assert `peak_record_buffer_bytes <= max_record_bytes + 1`.

- [x] **Step 2: Run the new integration test and observe RED**

Run:

```bash
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_jsonl_record_stream -- --nocapture
```

Expected: compile failure because `jsonl_record_stream` and `visit_jsonl_records` do not exist.

- [x] **Step 3: Implement bounded decoding**

Use `BufRead::read_until(b'\n', ...)` with an explicit length check after every append. For tar.gz, expose each matching regular member through the tar entry's limited reader; consume and count nonmatching members without allocating their contents. For ZIP, require exactly one JSONL regular file and read it through the same bounded line loop. Count all decompressed bytes and fail as soon as any configured bound is exceeded. Do not build `String`, `Vec<TarMember>`, or `Vec<record>` collections.

- [x] **Step 4: Run tests to GREEN**

Run:

```bash
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_jsonl_record_stream -- --nocapture
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --lib tar_reader::tests -- --nocapture
```

Expected: all container and fail-closed cases pass, including the measured peak record buffer assertion.

- [x] **Step 5: Commit the bounded visitor**

```bash
git add crates/backtesting-vertical-slice/src/jsonl_record_stream.rs crates/backtesting-vertical-slice/src/tar_reader.rs crates/backtesting-vertical-slice/src/lib.rs crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_jsonl_record_stream.rs crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs
git commit -m "feat(backtest): stream bounded JSONL archives"
```

### Task 3: Compile seeded level-set archives into a bounded replay window

**Files:**
- Create: `crates/backtesting-vertical-slice/src/seeded_level_set_deltas.rs`
- Modify: `crates/backtesting-vertical-slice/src/canonical_order_book_deltas.rs`
- Modify: `crates/backtesting-vertical-slice/src/canonical_trades.rs`
- Modify: `crates/backtesting-vertical-slice/src/operator.rs`
- Modify: `crates/backtesting-vertical-slice/src/lib.rs`
- Create: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_seeded_level_set_deltas.rs`
- Modify: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs`

**Interfaces:**
- Consumes: an `AcceptedDataset`, the existing `CanonicalInstrumentIdentity`, an NT `InstrumentAny` built from the run-spec's existing `CatalogInstrumentSpec`, explicit run-window bounds copied from `BacktestingRunManifest.start_time/end_time`, the existing `RawPayloadConfig`, and one `SeededLevelSetMappingConfig` selected by TOML.
- Produces:

```rust
pub enum SourceSequencePolicy {
    Native { path: Vec<String> },
    Unavailable,
}

pub enum OrderCountPolicy {
    Absent,
    ValidateNonNegativeAndDrop { index: usize },
}

// Nested under RawPayloadConfig as `jsonl_stream` so every bounded JSONL
// adapter uses one TOML-owned limit group.
pub struct JsonlStreamConfig {
    pub max_members: u64,
    pub max_record_bytes: usize,
    pub max_records: u64,
}

pub struct SeededLevelSetOutputLimits {
    pub max_levels_per_event: usize,
    pub max_active_levels_per_side: usize,
    pub max_selected_events: u64,
    pub max_selected_delta_rows: u64,
    pub max_emitted_bytes: u64,
}

pub struct SeededLevelSetWindow {
    pub deltas: CanonicalOrderBookDeltasTable,
    pub quotes: Option<CanonicalQuotesTable>,
    pub scan: JsonlScanStats,
}

pub fn normalize_seeded_level_set_window(
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    instrument: &InstrumentAny,
    window: SeededLevelSetWindowBounds,
    raw_payload: &RawPayloadConfig,
    config: &SeededLevelSetMappingConfig,
) -> Result<SeededLevelSetWindow>;
```

`JsonlStreamLimits` is constructed internally from `RawPayloadConfig.max_decoded_bytes`,
`max_member_bytes`, `member_suffix`, and `RawPayloadConfig.jsonl_stream`; those limits
are never duplicated. `OrderCountPolicy::ValidateNonNegativeAndDrop` is an explicit
converter-config field, so the unavoidable NT `OrderBookDelta` count loss is bound
by the existing `converter_config_hash` in `ConversionFingerprint`/`ConversionManifest`.

- [x] **Step 1: Write failing OKX- and Bybit-shaped tests**

Use generic configs, not venue branches. Assert identity-path equality, exact tuple arity, nonnegative order-count validation, update-before-snapshot rejection, timestamp regression rejection, invalid action rejection, active-book and output cap failures, archive-order replay, and all required flags/actions. Later snapshots are valid authoritative re-seeds. Assert OKX-shaped unavailable source sequence projects to NT `0`; Bybit-shaped native sequence is shared by every delta in its source event.

- [x] **Step 2: Write failing event-window and quote tests**

Create records before, within, and after a manifest window. Assert the output begins with one reconstructed seed snapshot for the pre-window book, contains only in-window source events, scans through EOF for malformed-tail detection, and emits no quote for the seed. Assert one quote per selected source event when both sides exist, exact event/availability timestamps, and no quote while a side is absent.

- [x] **Step 3: Run the new integration test and observe RED**

Run:

```bash
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_seeded_level_set_deltas -- --nocapture
```

Expected: compile failure because the seeded level-set API does not exist.

- [x] **Step 4: Implement the minimal converter**

Parse configured paths and indices, compare each record's configured identity value to `identity.venue_symbol`, and validate all configured limits before mutation. Maintain one NT `OrderBook` in L2/MBP mode. Apply source events atomically in archive order. Before the first selected event, call NT's snapshot surface to serialize the prior state into canonical seed rows; then emit canonical rows for selected events and derive one quote from the final NT book state. The converter assigns dense canonical row ordinals, carries native source-event sequence separately, stamps per-row `availability_time`, and records order-count loss in the transform evidence.

- [x] **Step 5: Project to the existing catalog and prove replay**

Project only through `project_canonical_order_book_deltas_to_catalog`, read the selected catalog window back, reassemble groups by `F_LAST`, apply them through NT `OrderBook::apply_deltas`, and compare final depth/BBO with the converter result. No new catalog writer is introduced.

- [x] **Step 6: Run focused tests to GREEN**

Run:

```bash
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_seeded_level_set_deltas -- --nocapture
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_order_book_delta_projection -- --nocapture
```

Expected: all positive, fail-closed, roundtrip, and BBO cases pass.

- [x] **Step 7: Commit the canonical seeded family**

```bash
git add crates/backtesting-vertical-slice/src/seeded_level_set_deltas.rs crates/backtesting-vertical-slice/src/canonical_order_book_deltas.rs crates/backtesting-vertical-slice/src/canonical_trades.rs crates/backtesting-vertical-slice/src/operator.rs crates/backtesting-vertical-slice/src/lib.rs crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_seeded_level_set_deltas.rs crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs
git commit -m "feat(backtest): ingest bounded full-depth L2 windows"
```

### Task 4: Prove issue-789 equivalence and delete the old authority path

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Modify: `crates/backtesting-vertical-slice/src/operator.rs`
- Modify: `crates/backtesting-vertical-slice/src/canonical_trades.rs`
- Modify: `crates/backtesting-vertical-slice/src/lib.rs`
- Delete: `crates/backtesting-vertical-slice/src/seeded_l2_quotes.rs`
- Test: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_seeded_level_set_deltas.rs`

**Interfaces:**
- Consumes: `SeededLevelSetWindow` and the existing SHA-pinned issue-789 OKX/Bybit fixtures.
- Produces: one operator route for full-depth deltas and BBO; no `SourceAdapterKind::SeededL2Quotes`, `[converter.seeded_l2_quotes]`, or direct legacy normalizer.

- [x] **Step 1: Capture immutable semantic goldens from the accepted fixtures**

Before deleting the legacy module, compute fixture-local golden values over quote instrument, bid/ask price/size, event time, availability time, source sequence, row count, and ordering. Store only counts and SHA-256 literals in tests; do not retain a second implementation.

- [x] **Step 2: Add differential tests**

Run both fixtures through the new canonical-delta path and assert the golden semantic digest and row count. Add a one-sided synthetic fixture to pin the existing both-sides-present rule. Assert catalog replay yields identical quote semantics.

Run:

```bash
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --lib runner::tests::issue_789 -- --nocapture
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_seeded_level_set_deltas -- --nocapture
```

Expected: the canonical-delta-derived quotes match the immutable legacy counts and semantic hashes.

- [x] **Step 3: Switch the production operator and runner callers**

Select the registered delta-family adapter through `[converter.seeded_level_set]`, pass the manifest window and TOML limits, project the canonical table through the existing bridge, and use the derived quotes for the issue-789 feed. Support all four containers through the shared record visitor.

- [x] **Step 4: Delete the old path completely**

Delete the module and remove its enum variant, converter section, registry lookup, normalizer entry points, wrong-kind fence arms, operator dispatch, re-export, execution-pack references, and runner literals. Use compiler errors and `rg` only for inventory; do not add source-scanning tests.

- [x] **Step 5: Run the equivalence and deletion proof to GREEN**

Run:

```bash
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --lib runner::tests::issue_789 -- --nocapture
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_seeded_level_set_deltas -- --nocapture
rg -n 'SeededL2Quotes|seeded_l2_quotes|SEEDED_L2_QUOTES_ADAPTER' crates/backtesting-vertical-slice
```

Expected: tests pass and the inventory search returns no matches.

- [x] **Step 6: Commit the cutover and deletion**

```bash
git add -A crates/backtesting-vertical-slice
git commit -m "refactor(backtest): remove seeded quote dual path"
```

### Task 5: End-to-end evidence, adversarial review, and PR publication

**Files:**
- Modify: `docs/superpowers/plans/2026-08-20-historical-full-depth-l2.md` only to check completed steps and record exact commands; no transient CI status in the PR body.

**Interfaces:**
- Consumes: the final single-path implementation and representative real/source-shaped archives.
- Produces: exact-head local evidence, a clean pushed branch, a code PR, and a native review request to node ID `U_kgDOEZMFhA`'s current login.

- [x] **Step 1: Run formatting and focused verification**

```bash
cargo fmt --manifest-path crates/backtesting-vertical-slice/Cargo.toml --check
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_jsonl_record_stream -- --nocapture
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_seeded_level_set_deltas -- --nocapture
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --test backtesting_vertical_slice_tests backtesting_vertical_slice_order_book_delta_projection -- --nocapture
cargo test --locked --manifest-path crates/backtesting-vertical-slice/Cargo.toml --lib runner::tests::issue_789 -- --nocapture
git diff --check origin/main...HEAD
```

Expected: every command exits zero.

Exact-head verification passed after rebasing onto `origin/main`: formatting, seeded-level-set unit tests, tar-reader unit tests (including header checksum and two-block end-marker rejection), bounded JSONL record-stream tests, order-book-delta catalog projection tests, seeded-level-set operator tests, L2 manifest tests, library Clippy with warnings denied, and the full issue-789 replay over 446,717 data elements. The real-fixture regression preserved 15,148 OKX and 2,719 Bybit derived BBO rows at their pinned semantic hashes.

- [x] **Step 2: Run class-level boundedness and malformed-tail evidence**

Use the generated large single-member archive test to report input bytes, selected event window, peak record buffer, active-level peak, selected rows, and catalog bytes. Confirm a malformed record after the selected window makes the whole conversion fail and publishes no authoritative catalog.

Measured generated evidence: 9,600,017 input bytes / 100,000 records, one selected event, 113-byte peak record buffer, one active level per side, four selected delta rows, 4,280 emitted row bytes, 5,304 serialized output bytes, and 9,744 catalog bytes. The operator malformed-tail test rejects the conversion before the catalog root exists.

- [x] **Step 3: Run a current-state internal adversarial review**

Review the exact diff for a second raw-to-catalog writer, retained legacy entry point, unbounded collection, venue conditional, hardcoded runtime value, synthetic sequence, false L3 claim, undeclared order-count loss, `ts_init` collapse, event splitting, or non-window output. Resolve every substantive finding before publication.

- [x] **Step 4: Commit any final evidence-only corrections and verify a clean head**

```bash
git status --short
git diff --check origin/main...HEAD
```

Expected: the worktree is clean and the diff contains code plus its behavioral evidence, not a docs-only change.

- [x] **Step 5: Push and open the code PR**

Publish one commit through the repository's Mergify stack hook after a dry run proves it will create exactly one PR. Open one PR naming this as the historical full-depth L2 slice, state remaining broader data-provider/on-chain scope as out of scope, resolve node ID `U_kgDOEZMFhA` to the current login, and request that reviewer. Report the exact head SHA and detach; do not merge without explicit user authorization and that native approval.

Published as code PR #1557 with the mandated reviewer request active. Mergify publication used one squashed commit so this slice remains one PR rather than a commit-per-PR stack.
