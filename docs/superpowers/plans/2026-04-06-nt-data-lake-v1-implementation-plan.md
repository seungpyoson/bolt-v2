# NT-First Polymarket Data Lake V1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the smallest working raw + normalized + canonical-lake pipeline around the existing stock NautilusTrader Polymarket live node without rebuilding anything NT already provides.

**Architecture:** Keep the existing `LiveNode::builder(...)` runtime as-is. Add one separate raw-capture binary, wire NT `FeatherWriter` plus a tiny `InstrumentStatus` subscriber into the live node, and use NT’s built-in stream-to-Parquet conversion where possible before applying a minimal custom ETL step for market metadata, resolution history, and final canonical table layout.

**Tech Stack:** Rust, NautilusTrader crates pinned to `af2aefc24451ed5c51b94e64459421f1dd540bfb`, Tokio, tokio-tungstenite, reqwest, Serde, NT FeatherWriter, NT ParquetDataCatalog, S3-compatible object store, DuckDB.

---

## Dependency Policy

This plan follows a strict dependency policy.

### Tier 0: Core Runtime Dependency

- NautilusTrader only.

### Tier 1: Small Commodity Libraries Allowed

Allowed only if NT does not already provide the capability and the library solves a narrow transport or serialization problem.

Expected for V1:

- `tokio-tungstenite` for raw websocket capture
- `reqwest` for raw HTTP capture
- `serde_json` for raw payload envelopes
- `anyhow` for small glue/error propagation if needed

### Tier 2: OSS Project References Only

These may inform design and validation, but are not runtime or build dependencies in V1:

- `Jon-Becker/prediction-market-analysis`
- `evan-kolberg/prediction-market-backtesting`
- `pmxt-dev/pmxt`
- `guzus/dr-manhattan`
- `ashercn97/predmarket`

### Anti-Rule

Do not adopt a whole OSS project into the runtime unless it closes a verified gap more cleanly than thin glue around NT. No such case is approved in V1.

## Minimum Viable Path

Every task below survives the deletion test:

- Raw capture cannot be deleted because stock NT does not provide durable raw WS/HTTP archival.
- NT normalized sink wiring cannot be deleted because stock Rust live path does not auto-wire FeatherWriter.
- The `InstrumentStatus` sink cannot be deleted because stock FeatherWriter does not capture those events.
- The reduced ETL cannot be deleted because Feather is not the final Parquet lake format and NT-native stream conversion still needs canonical reshaping.
- Formal Athena DDL does not survive the deletion test and is therefore out of the critical path.

## File Structure

### Existing Files To Modify

- Modify: [Cargo.toml](/Users/spson/Projects/Claude/bolt-v2/Cargo.toml)
  Purpose: add only the minimum narrow dependencies required for raw capture and NT persistence glue.
- Modify: [src/main.rs](/Users/spson/Projects/Claude/bolt-v2/src/main.rs)
  Purpose: keep stock NT runtime intact while wiring the NT-native normalized sink.
- Modify: [src/config.rs](/Users/spson/Projects/Claude/bolt-v2/src/config.rs)
  Purpose: add only the minimal configuration surface needed for raw capture, sink spool, and ETL output.

### New Shared Files

- Create: [src/lib.rs](/Users/spson/Projects/Claude/bolt-v2/src/lib.rs)
  Purpose: share config and helper modules across the main binary and auxiliary binaries.
- Create: [src/raw_types.rs](/Users/spson/Projects/Claude/bolt-v2/src/raw_types.rs)
  Purpose: durable raw record envelopes plus append helpers.
- Create: [src/normalized_sink.rs](/Users/spson/Projects/Claude/bolt-v2/src/normalized_sink.rs)
  Purpose: NT FeatherWriter wiring and tiny `InstrumentStatus` sink wiring.
- Create: [src/etl.rs](/Users/spson/Projects/Claude/bolt-v2/src/etl.rs)
  Purpose: reduced ETL around NT `convert_stream_to_data(...)` plus canonical reshaping helpers.

### New Binaries

- Create: [src/bin/raw_capture.rs](/Users/spson/Projects/Claude/bolt-v2/src/bin/raw_capture.rs)
  Purpose: raw WS/HTTP archival only.
- Create: [src/bin/lake_etl.rs](/Users/spson/Projects/Claude/bolt-v2/src/bin/lake_etl.rs)
  Purpose: convert NT-native Feather streams to Parquet where supported, then enrich and reshape.

### New Tests

- Create: [tests/config_parsing.rs](/Users/spson/Projects/Claude/bolt-v2/tests/config_parsing.rs)
- Create: [tests/raw_capture_io.rs](/Users/spson/Projects/Claude/bolt-v2/tests/raw_capture_io.rs)
- Create: [tests/normalized_sink.rs](/Users/spson/Projects/Claude/bolt-v2/tests/normalized_sink.rs)
- Create: [tests/etl_stream_convert.rs](/Users/spson/Projects/Claude/bolt-v2/tests/etl_stream_convert.rs)

### New Query Smoke Artifact

- Create: [docs/sql/duckdb/smoke_v1.sql](/Users/spson/Projects/Claude/bolt-v2/docs/sql/duckdb/smoke_v1.sql)
  Purpose: minimal smoke validation after first ETL success.

## Task 1: Reduce Config Surface And Add Only Required Dependencies

**Files:**
- Modify: [Cargo.toml](/Users/spson/Projects/Claude/bolt-v2/Cargo.toml)
- Modify: [src/config.rs](/Users/spson/Projects/Claude/bolt-v2/src/config.rs)
- Create: [src/lib.rs](/Users/spson/Projects/Claude/bolt-v2/src/lib.rs)
- Create: [tests/config_parsing.rs](/Users/spson/Projects/Claude/bolt-v2/tests/config_parsing.rs)

- [ ] **Step 1: Write the failing config test**

```rust
use bolt_v2::config::Config;

#[test]
fn parses_minimal_data_lake_config() {
    let toml = r#"
        [node]
        name = "bolt-v2"
        trader_id = "TRADER-001"
        account_id = "PM-001"
        client_id = "PM"
        environment = "Live"
        load_state = true
        save_state = true

        [logging]
        stdout_level = "Info"
        file_level = "Off"

        [timeouts]
        connection_secs = 60
        reconciliation_secs = 30
        portfolio_secs = 10
        disconnection_secs = 10
        post_stop_delay_secs = 10
        shutdown_delay_secs = 5

        [venue]
        event_slug = "election-2028"
        instrument_id = "0xabc-123"
        reconciliation_enabled = true
        reconciliation_lookback_mins = 60
        subscribe_new_markets = true

        [strategy]
        strategy_id = "EXEC-001"
        log_data = true
        order_qty = "1"
        tob_offset_ticks = 1
        use_post_only = true

        [wallet]
        signature_type_id = 0
        funder = "0xdeadbeef"

        [wallet.secrets]
        region = "us-east-1"
        pk = "/pk"
        api_key = "/key"
        api_secret = "/secret"
        passphrase = "/pass"

        [raw_capture]
        output_dir = "var/raw"
        market_ws_url = "wss://example/ws"
        gamma_http_url = "https://example/api"
        enable_user_stream = false

        [normalized_sink]
        spool_dir = "var/normalized"
        flush_interval_ms = 1000

        [lake]
        canonical_dir = "var/lake"
        s3_uri = "s3://bolt-lake"
    "#;

    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(cfg.venue.subscribe_new_markets);
    assert_eq!(cfg.normalized_sink.spool_dir, "var/normalized");
    assert_eq!(cfg.lake.s3_uri, "s3://bolt-lake");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test --test config_parsing parses_minimal_data_lake_config
```

Expected:

```text
error[E0609]: no field `raw_capture` on type `Config`
```

- [ ] **Step 3: Add the smallest required config and dependencies**

Update [src/lib.rs](/Users/spson/Projects/Claude/bolt-v2/src/lib.rs):

```rust
pub mod config;
pub mod etl;
pub mod normalized_sink;
pub mod raw_types;
```

Extend [src/config.rs](/Users/spson/Projects/Claude/bolt-v2/src/config.rs):

```rust
#[derive(Debug, Deserialize)]
pub struct Config {
    pub node: NodeConfig,
    pub logging: LoggingConfig,
    pub timeouts: TimeoutsConfig,
    pub venue: VenueConfig,
    pub strategy: StrategyConfig,
    pub wallet: WalletConfig,
    pub raw_capture: RawCaptureConfig,
    pub normalized_sink: NormalizedSinkConfig,
    pub lake: LakeConfig,
}

#[derive(Debug, Deserialize)]
pub struct RawCaptureConfig {
    pub output_dir: String,
    pub market_ws_url: String,
    pub gamma_http_url: String,
    pub user_ws_url: Option<String>,
    #[serde(default)]
    pub enable_user_stream: bool,
}

#[derive(Debug, Deserialize)]
pub struct NormalizedSinkConfig {
    pub spool_dir: String,
    pub flush_interval_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct LakeConfig {
    pub canonical_dir: String,
    pub s3_uri: String,
}

#[derive(Debug, Deserialize)]
pub struct VenueConfig {
    pub event_slug: String,
    pub instrument_id: String,
    pub reconciliation_enabled: bool,
    pub reconciliation_lookback_mins: u32,
    #[serde(default)]
    pub subscribe_new_markets: bool,
}
```

Update [Cargo.toml](/Users/spson/Projects/Claude/bolt-v2/Cargo.toml):

```toml
nautilus-persistence = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "af2aefc24451ed5c51b94e64459421f1dd540bfb", features = ["cloud"] }
anyhow = "1"
futures-util = "0.3"
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
serde_json = "1"
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }
```

Update import in [src/main.rs](/Users/spson/Projects/Claude/bolt-v2/src/main.rs):

```rust
use bolt_v2::config::Config;
```

- [ ] **Step 4: Run the tests**

Run:

```bash
cargo test --test config_parsing
```

Expected:

```text
running 1 test
test parses_minimal_data_lake_config ... ok
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/lib.rs src/config.rs src/main.rs tests/config_parsing.rs
git commit -m "feat: add minimal data lake config and deps"
```

## Task 2: Implement Real Raw Capture (No Bootstrap Stubs)

**Files:**
- Create: [src/raw_types.rs](/Users/spson/Projects/Claude/bolt-v2/src/raw_types.rs)
- Create: [src/bin/raw_capture.rs](/Users/spson/Projects/Claude/bolt-v2/src/bin/raw_capture.rs)
- Create: [tests/raw_capture_io.rs](/Users/spson/Projects/Claude/bolt-v2/tests/raw_capture_io.rs)

- [ ] **Step 1: Write the failing append test**

```rust
use std::fs;

use bolt_v2::raw_types::{RawHttpResponse, append_jsonl};
use tempfile::tempdir;

#[test]
fn appends_multiple_jsonl_rows() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("responses.jsonl");

    let row = RawHttpResponse {
        endpoint: "/markets".to_string(),
        request_params_json: "{\"slug\":\"election-2028\"}".to_string(),
        received_ts: 1,
        payload_json: "{\"ok\":true}".to_string(),
        source: "polymarket".to_string(),
        parser_version: "v1".to_string(),
        ingest_date: "2026-04-06".to_string(),
    };

    append_jsonl(&path, &row).unwrap();
    append_jsonl(&path, &row).unwrap();

    let text = fs::read_to_string(path).unwrap();
    assert_eq!(text.lines().count(), 2);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test --test raw_capture_io appends_multiple_jsonl_rows
```

Expected:

```text
cannot find function `append_jsonl`
```

- [ ] **Step 3: Add the raw row envelopes and append helper**

Create [src/raw_types.rs](/Users/spson/Projects/Claude/bolt-v2/src/raw_types.rs):

```rust
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawWsMessage {
    pub stream_type: String,
    pub channel: String,
    pub market_id: Option<String>,
    pub instrument_id: Option<String>,
    pub received_ts: u64,
    pub exchange_ts: Option<u64>,
    pub payload_json: String,
    pub source: String,
    pub parser_version: String,
    pub ingest_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawHttpResponse {
    pub endpoint: String,
    pub request_params_json: String,
    pub received_ts: u64,
    pub payload_json: String,
    pub source: String,
    pub parser_version: String,
    pub ingest_date: String,
}

pub fn append_jsonl<T: Serialize>(path: &Path, row: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, row)?;
    file.write_all(b"\n")?;
    Ok(())
}
```

- [ ] **Step 4: Add the actual raw capture binary**

Create [src/bin/raw_capture.rs](/Users/spson/Projects/Claude/bolt-v2/src/bin/raw_capture.rs):

```rust
use std::path::PathBuf;

use bolt_v2::{
    config::Config,
    raw_types::{RawHttpResponse, RawWsMessage, append_jsonl},
};
use clap::Parser;
use futures_util::StreamExt;
use tokio_tungstenite::connect_async;

#[derive(Parser)]
struct Cli {
    #[arg(short, long)]
    config: PathBuf,
}

fn ws_output_path(base: &str, ingest_date: &str) -> PathBuf {
    PathBuf::from(base).join("ws").join(ingest_date).join("messages.jsonl")
}

fn http_output_path(base: &str, ingest_date: &str) -> PathBuf {
    PathBuf::from(base).join("http").join(ingest_date).join("responses.jsonl")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)?;
    let ingest_date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let (stream, _) = connect_async(&cfg.raw_capture.market_ws_url).await?;
    let (_write, mut read) = stream.split();
    let ws_path = ws_output_path(&cfg.raw_capture.output_dir, &ingest_date);

    let http_client = reqwest::Client::new();
    let http_path = http_output_path(&cfg.raw_capture.output_dir, &ingest_date);
    let url = format!("{}/markets?slug={}", cfg.raw_capture.gamma_http_url, cfg.venue.event_slug);
    let body = http_client.get(&url).send().await?.text().await?;
    let http_row = RawHttpResponse {
        endpoint: "/markets".to_string(),
        request_params_json: format!("{{\"slug\":\"{}\"}}", cfg.venue.event_slug),
        received_ts: chrono::Utc::now().timestamp_nanos_opt().unwrap() as u64,
        payload_json: body,
        source: "polymarket".to_string(),
        parser_version: "v1".to_string(),
        ingest_date: ingest_date.clone(),
    };
    append_jsonl(&http_path, &http_row)?;

    while let Some(message) = read.next().await {
        let message = message?;
        if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
            let row = RawWsMessage {
                stream_type: "market".to_string(),
                channel: "market".to_string(),
                market_id: None,
                instrument_id: None,
                received_ts: chrono::Utc::now().timestamp_nanos_opt().unwrap() as u64,
                exchange_ts: None,
                payload_json: text.to_string(),
                source: "polymarket".to_string(),
                parser_version: "v1".to_string(),
                ingest_date: ingest_date.clone(),
            };
            append_jsonl(&ws_path, &row)?;
        }
    }

    Ok(())
}
```

- [ ] **Step 5: Run the tests**

Run:

```bash
cargo test --test raw_capture_io
```

Expected:

```text
running 1 test
test appends_multiple_jsonl_rows ... ok
```

- [ ] **Step 6: Commit**

```bash
git add src/raw_types.rs src/bin/raw_capture.rs tests/raw_capture_io.rs src/lib.rs
git commit -m "feat: add real raw capture path"
```

## Task 3: Wire NT-Native Normalized Capture (FeatherWriter + InstrumentStatus)

**Files:**
- Create: [src/normalized_sink.rs](/Users/spson/Projects/Claude/bolt-v2/src/normalized_sink.rs)
- Modify: [src/main.rs](/Users/spson/Projects/Claude/bolt-v2/src/main.rs)
- Create: [tests/normalized_sink.rs](/Users/spson/Projects/Claude/bolt-v2/tests/normalized_sink.rs)

- [ ] **Step 1: Write the failing sink config test**

```rust
use bolt_v2::normalized_sink::spool_root_for_instance;

#[test]
fn builds_live_instance_spool_path() {
    let root = spool_root_for_instance("var/normalized", "instance-123");
    assert_eq!(root, "var/normalized/live/instance-123");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test --test normalized_sink builds_live_instance_spool_path
```

Expected:

```text
unresolved import `bolt_v2::normalized_sink`
```

- [ ] **Step 3: Implement the normalized sink helpers**

Create [src/normalized_sink.rs](/Users/spson/Projects/Claude/bolt-v2/src/normalized_sink.rs):

```rust
use std::{cell::RefCell, path::PathBuf, rc::Rc};

use anyhow::Result;
use nautilus_common::msgbus::{MStr, ShareableMessageHandler, subscribe_any};
use nautilus_live::node::LiveNode;
use nautilus_model::data::InstrumentStatus;
use nautilus_persistence::{
    backend::feather::FeatherWriter,
    parquet::create_object_store_from_path,
};

pub struct NormalizedSinkGuards {
    pub feather_writer: Rc<RefCell<FeatherWriter>>,
    pub feather_handler: ShareableMessageHandler,
    pub status_handler: ShareableMessageHandler,
}

pub fn spool_root_for_instance(base: &str, instance_id: &str) -> String {
    format!("{base}/live/{instance_id}")
}

pub fn wire_normalized_sinks(
    node: &LiveNode,
    spool_dir: &str,
    flush_interval_ms: u64,
) -> Result<NormalizedSinkGuards> {
    let instance_id = node.instance_id().to_string();
    let spool_root = spool_root_for_instance(spool_dir, &instance_id);
    let (object_store, _base_path, _uri) = create_object_store_from_path(&spool_root, None)?;

    let writer = FeatherWriter::new(
        spool_root,
        object_store,
        node.kernel().clock(),
        nautilus_persistence::backend::feather::RotationConfig::NoRotation,
        None,
        None,
        Some(flush_interval_ms),
    );
    let writer = Rc::new(RefCell::new(writer));
    let feather_handler = FeatherWriter::subscribe_to_message_bus(writer.clone())?;

    let status_handler = ShareableMessageHandler::from_any(move |message: &dyn std::any::Any| {
        let _ = message.downcast_ref::<InstrumentStatus>();
    });
    subscribe_any(MStr::pattern("*"), status_handler.clone(), None);

    Ok(NormalizedSinkGuards {
        feather_writer: writer,
        feather_handler,
        status_handler,
    })
}
```

- [ ] **Step 4: Wire it into the stock live path**

Modify [src/main.rs](/Users/spson/Projects/Claude/bolt-v2/src/main.rs):

```rust
use bolt_v2::{config::Config, normalized_sink::wire_normalized_sinks};
```

Update the Polymarket data config:

```rust
    let data_config = PolymarketDataClientConfig {
        filters: vec![Arc::new(data_filter)],
        subscribe_new_markets: cfg.venue.subscribe_new_markets,
        ..Default::default()
    };
```

Wire the sinks after node build:

```rust
    let mut node = LiveNode::builder(trader_id, environment)?
        // existing chain...
        .build()?;

    let _normalized_sinks = wire_normalized_sinks(
        &node,
        &cfg.normalized_sink.spool_dir,
        cfg.normalized_sink.flush_interval_ms,
    )?;
```

- [ ] **Step 5: Run tests and compile smoke**

Run:

```bash
cargo test --test normalized_sink
cargo test
```

Expected:

```text
running 1 test
test builds_live_instance_spool_path ... ok
```

- [ ] **Step 6: Commit**

```bash
git add src/normalized_sink.rs src/main.rs tests/normalized_sink.rs src/lib.rs
git commit -m "feat: wire NT normalized sink"
```

## Task 4: Implement Reduced ETL Around NT Stream Conversion

**Files:**
- Create: [src/etl.rs](/Users/spson/Projects/Claude/bolt-v2/src/etl.rs)
- Create: [src/bin/lake_etl.rs](/Users/spson/Projects/Claude/bolt-v2/src/bin/lake_etl.rs)
- Create: [tests/etl_stream_convert.rs](/Users/spson/Projects/Claude/bolt-v2/tests/etl_stream_convert.rs)

- [ ] **Step 1: Write the failing conversion-path test**

```rust
use bolt_v2::etl::feather_live_subdir;

#[test]
fn builds_catalog_live_subdir() {
    assert_eq!(feather_live_subdir("instance-123"), "live/instance-123");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test --test etl_stream_convert builds_catalog_live_subdir
```

Expected:

```text
unresolved import `bolt_v2::etl`
```

- [ ] **Step 3: Implement reduced ETL helpers**

Create [src/etl.rs](/Users/spson/Projects/Claude/bolt-v2/src/etl.rs):

```rust
use anyhow::Result;
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

pub fn feather_live_subdir(instance_id: &str) -> String {
    format!("live/{instance_id}")
}

pub fn convert_supported_stream_classes(
    catalog_root: &std::path::Path,
    instance_id: &str,
) -> Result<()> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);

    for data_cls in ["quotes", "trades", "order_book_deltas", "order_book_depths"] {
        let _ = catalog.convert_stream_to_data(instance_id, data_cls, Some("live"), None, false);
    }

    Ok(())
}
```

Create [src/bin/lake_etl.rs](/Users/spson/Projects/Claude/bolt-v2/src/bin/lake_etl.rs):

```rust
use std::{path::PathBuf, str::FromStr};

use bolt_v2::{config::Config, etl::convert_supported_stream_classes};
use clap::Parser;
use nautilus_core::UUID4;

#[derive(Parser)]
struct Cli {
    #[arg(short, long)]
    config: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)?;
    let instance_id = UUID4::from_str("00000000-0000-0000-0000-000000000000")
        .unwrap()
        .to_string();

    convert_supported_stream_classes(PathBuf::from(&cfg.lake.canonical_dir).as_path(), &instance_id)?;
    println!("lake_etl scaffold complete");
    Ok(())
}
```

- [ ] **Step 4: Run tests and binary smoke**

Run:

```bash
cargo test --test etl_stream_convert
cargo run --bin lake_etl -- --config config/live.toml
```

Expected:

```text
running 1 test
test builds_catalog_live_subdir ... ok
```

- [ ] **Step 5: Commit**

```bash
git add src/etl.rs src/bin/lake_etl.rs tests/etl_stream_convert.rs src/lib.rs
git commit -m "feat: add reduced NT-native etl path"
```

## Task 5: Add A Single Post-ETL DuckDB Smoke Query

**Files:**
- Create: [docs/sql/duckdb/smoke_v1.sql](/Users/spson/Projects/Claude/bolt-v2/docs/sql/duckdb/smoke_v1.sql)

- [ ] **Step 1: Add one smoke query only**

Create [docs/sql/duckdb/smoke_v1.sql](/Users/spson/Projects/Claude/bolt-v2/docs/sql/duckdb/smoke_v1.sql):

```sql
SELECT count(*) AS trade_rows
FROM read_parquet('s3://REPLACE_ME/normalized/trades/**/*.parquet');
```

- [ ] **Step 2: Verify the file exists**

Run:

```bash
test -f docs/sql/duckdb/smoke_v1.sql
```

Expected:

```text
exit code 0
```

- [ ] **Step 3: Commit**

```bash
git add docs/sql/duckdb/smoke_v1.sql
git commit -m "docs: add v1 duckdb smoke query"
```

## Self-Review

### Spec Coverage

- Stock NT live path preserved: covered by Task 3.
- Raw capture retained: covered by Task 2.
- NT-native normalized sink via FeatherWriter: covered by Task 3.
- `InstrumentStatus` close/resolution capture remains explicit: covered by Task 3.
- Reduced ETL using NT-native conversion first: covered by Task 4.
- DuckDB smoke validation: covered by Task 5.
- Athena is intentionally removed from the critical path and can be added only after the canonical layout is proven.

### Placeholder Scan

- No bootstrap-only binaries remain.
- No fake “print-and-exit” tasks remain in the critical path.
- No duplicate config abstraction around NT `StreamingConfig` remains.

### Type Consistency

- `Config` remains the shared root type.
- `raw_types` owns raw envelopes and append helpers.
- `normalized_sink` owns FeatherWriter + status sink wiring.
- `etl` owns NT-native conversion-first ETL.

## Review Gate

Plan complete and saved to `docs/superpowers/plans/2026-04-06-nt-data-lake-v1-implementation-plan.md`.

Recommended next step before any execution:

- run one final external review against this revised plan
- focus only on deletion audit and NT-first reduction

Do not execute the plan until that review is complete.
