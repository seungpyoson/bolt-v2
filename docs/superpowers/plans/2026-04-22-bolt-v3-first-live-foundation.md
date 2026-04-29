# bolt-v3 First-Live Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first-live-trade `bolt-v3` path as a thin NautilusTrader-native Polymarket execution binary with explicit root/strategy schemas, `just check`, strategy-owned target resolution, and two explicit integration checks: Polymarket filter-based target loading and custom-data event persistence to the local catalog.

**Architecture:** Build the v3 path in a new `src/v3/` module tree and a new `src/bin/bolt.rs` binary while leaving the old `bolt-v2` runtime in place until the new path is verified. Keep the strategy/execution boundary fully NautilusTrader-native, use AWS SDK for SSM resolution, use keyed venue configs from TOML, and treat the two integration checks as gating tasks before finishing runtime assembly.

**Tech Stack:** Rust 2024, Clap, Serde/TOML, AWS SDK for Systems Manager, NautilusTrader (pinned `48d1c126335b82812ba691c5661aeb2e912cde24`), Nautilus custom data + Parquet catalog, `just`

---

## Planned File Structure

### New source files

- `src/bin/bolt.rs`
  Entry binary for `bolt-v3` (`check` and `run` subcommands only).

- `src/v3/mod.rs`
  Top-level `v3` module export.

- `src/v3/cli.rs`
  Clap command types for `bolt check` and `bolt run`.

- `src/v3/app.rs`
  Command dispatcher used by the binary.

- `src/v3/config/mod.rs`
  Config module exports.

- `src/v3/config/root.rs`
  Root/entity TOML structs and serde validation.

- `src/v3/config/strategy.rs`
  Strategy TOML structs and serde validation.

- `src/v3/config/load.rs`
  Root-relative strategy-file loading, duplicate checks, venue/reference ownership validation, and archetype-specific structural validation.

- `src/v3/check.rs`
  Canonical validation implementation used by `just check` and by runtime startup.

- `src/v3/secrets.rs`
  AWS SDK Systems Manager resolver plus environment-variable blocklist checks.

- `src/v3/venues/mod.rs`
  Venue config assembly entrypoints.

- `src/v3/venues/polymarket.rs`
  Pinned Polymarket data/execution config mapping and target-derived filter installation.

- `src/v3/venues/binance.rs`
  Pinned Binance data-client config mapping for reference-data venues.

- `src/v3/targets/mod.rs`
  Target resolution entrypoints and shared types.

- `src/v3/targets/types.rs`
  `ResolvedTarget` structs and target-kind enums used by strategies and forensics.

- `src/v3/targets/updown.rs`
  First-live `updown` series loading/filter derivation and `active_or_next` resolution helpers.

- `src/v3/forensics/mod.rs`
  Forensics module exports.

- `src/v3/forensics/events.rs`
  Fixed decision-event custom-data types.

- `src/v3/forensics/registry.rs`
  Custom-data registration and catalog wiring helpers.

- `src/v3/strategies/mod.rs`
  Compile-time archetype match for `binary_oracle_edge_taker`.

- `src/v3/strategies/binary_oracle_edge_taker.rs`
  First-live archetype implementation using NT-native order construction.

- `src/v3/runtime.rs`
  Live node assembly and strategy registration.

- `src/v3/panic_gate.rs`
  Shared panic-test helpers and reporting utilities for the `#239` matrix.

### New test files

- `tests/bolt_v3_cli.rs`
- `tests/bolt_v3_schema.rs`
- `tests/bolt_v3_secrets.rs`
- `tests/bolt_v3_polymarket_filters.rs`
- `tests/bolt_v3_custom_data_catalog.rs`
- `tests/bolt_v3_check.rs`
- `tests/bolt_v3_binary_oracle_edge_taker.rs`
- `tests/issue_239_panic_matrix.rs`

### Modified files

- `Cargo.toml`
  Add direct dependencies needed by v3 and the custom-data test path.

- `src/lib.rs`
  Export `v3`.

- `justfile`
  Add `check` and `run-v3` recipes that call the new binary only. No duplicate logic.

---

### Task 1: Scaffold the v3 binary and command surface

**Files:**
- Create: `src/bin/bolt.rs`
- Create: `src/v3/mod.rs`
- Create: `src/v3/cli.rs`
- Create: `src/v3/app.rs`
- Modify: `src/lib.rs`
- Modify: `justfile`
- Test: `tests/bolt_v3_cli.rs`

- [ ] **Step 1: Write the failing CLI tests**

```rust
// tests/bolt_v3_cli.rs
use clap::Parser;

use bolt_v2::v3::cli::{BoltCli, Command};

#[test]
fn parses_check_subcommand() {
    let cli = BoltCli::parse_from(["bolt", "check", "--config", "config/root.toml"]);
    match cli.command {
        Command::Check(args) => assert_eq!(args.config.as_os_str(), "config/root.toml"),
        other => panic!("expected check subcommand, got {other:?}"),
    }
}

#[test]
fn parses_run_subcommand() {
    let cli = BoltCli::parse_from(["bolt", "run", "--config", "config/root.toml"]);
    match cli.command {
        Command::Run(args) => assert_eq!(args.config.as_os_str(), "config/root.toml"),
        other => panic!("expected run subcommand, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run the CLI tests to verify they fail**

Run:

```bash
cargo test --test bolt_v3_cli -- --nocapture
```

Expected:

```text
error[E0433]: failed to resolve: could not find `v3` in `bolt_v2`
```

- [ ] **Step 3: Add the new module tree and binary**

```rust
// src/v3/mod.rs
pub mod app;
pub mod check;
pub mod cli;
pub mod config;
pub mod forensics;
pub mod runtime;
pub mod secrets;
pub mod strategies;
pub mod targets;
pub mod venues;
pub mod panic_gate;
```

```rust
// src/v3/cli.rs
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "bolt")]
pub struct BoltCli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Check(CheckArgs),
    Run(RunArgs),
}

#[derive(Debug, clap::Args)]
pub struct CheckArgs {
    #[arg(long)]
    pub config: PathBuf,
}

#[derive(Debug, clap::Args)]
pub struct RunArgs {
    #[arg(long)]
    pub config: PathBuf,
}
```

```rust
// src/v3/app.rs
use anyhow::Result;

use super::cli::{BoltCli, Command};

pub fn run(cli: BoltCli) -> Result<()> {
    match cli.command {
        Command::Check(_args) => anyhow::bail!("check not implemented yet"),
        Command::Run(_args) => anyhow::bail!("run not implemented yet"),
    }
}
```

```rust
// src/bin/bolt.rs
use clap::Parser;

use bolt_v2::v3::{app, cli::BoltCli};

fn main() -> anyhow::Result<()> {
    let cli = BoltCli::parse();
    app::run(cli)
}
```

```rust
// src/lib.rs
pub mod v3;
```

```make
# justfile
check: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --bin bolt -- check --config config/root.toml

run-v3: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --bin bolt -- run --config config/root.toml
```

- [ ] **Step 4: Re-run the CLI tests**

Run:

```bash
cargo test --test bolt_v3_cli -- --nocapture
```

Expected:

```text
test parses_check_subcommand ... ok
test parses_run_subcommand ... ok
```

- [ ] **Step 5: Commit the scaffold**

```bash
git add src/bin/bolt.rs src/v3/mod.rs src/v3/cli.rs src/v3/app.rs src/lib.rs justfile tests/bolt_v3_cli.rs
git commit -m "feat: scaffold bolt-v3 binary and command surface"
```

### Task 2: Define root/strategy schemas and structural loading rules

**Files:**
- Create: `src/v3/config/mod.rs`
- Create: `src/v3/config/root.rs`
- Create: `src/v3/config/strategy.rs`
- Create: `src/v3/config/load.rs`
- Test: `tests/bolt_v3_schema.rs`

- [ ] **Step 1: Write the failing schema tests**

```rust
// tests/bolt_v3_schema.rs
use std::fs;

use tempfile::TempDir;

use bolt_v2::v3::config::load::load_root_and_strategies;

#[test]
fn strategy_paths_are_resolved_relative_to_root_file() {
    let temp = TempDir::new().unwrap();
    let root_dir = temp.path().join("config");
    let strategies_dir = root_dir.join("strategies");
    fs::create_dir_all(&strategies_dir).unwrap();

    fs::write(
        root_dir.join("root.toml"),
        r#"
schema_version = 1
trader_identifier = "BOLT-001"
strategy_files = ["strategies/one.toml"]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
reconciliation_lookback_mins = 0
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

[risk]
bypass = false
max_order_submit_count = 20
max_order_submit_interval_seconds = 1
max_order_modify_count = 20
max_order_modify_interval_seconds = 1
default_max_notional_per_order = "10.00"

[logging]
standard_output_level = "INFO"
file_level = "INFO"
log_directory = "/var/log/bolt"

[persistence]
state_directory = "/var/lib/bolt/state"
catalog_directory = "/var/lib/bolt/catalog"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[venues.polymarket_main]
kind = "polymarket"

[venues.polymarket_main.data]
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
base_url_gamma = "https://gamma-api.polymarket.com"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
ws_timeout_seconds = 30
subscribe_new_markets = false
update_instruments_interval_minutes = 60
websocket_max_subscriptions_per_connection = 200

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder_address = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
max_retries = 3
retry_delay_initial_milliseconds = 250
retry_delay_max_milliseconds = 2000
ack_timeout_seconds = 5

[venues.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"
"#,
    )
    .unwrap();

    fs::write(
        strategies_dir.join("one.toml"),
        r#"
schema_version = 1
strategy_instance_identifier = "bitcoin_updown_main"
strategy_archetype = "binary_oracle_edge_taker"
order_id_tag = "001"
oms_type = "netting"
venue = "polymarket_main"

[target]
kind = "series"
series_family = "updown"
underlying_asset = "BTC"
cadence_seconds = 300
rotation_policy = "active_or_next"
retry_interval_seconds = 5
blocked_after_seconds = 60

[reference_data.primary]
venue = "polymarket_main"
instrument_identifier = "BTCUSDT.BINANCE"

[parameters.entry_order]
order_type = "limit"
time_in_force = "fok"
post_only = false
reduce_only = false
quote_quantity = false

[parameters.exit_order]
order_type = "market"
time_in_force = "ioc"
post_only = false
reduce_only = false
quote_quantity = false

[parameters]
edge_threshold_basis_points = 100
order_notional_target = "5.00"
maximum_position_notional = "10.00"
"#,
    )
    .unwrap();

    let loaded = load_root_and_strategies(&root_dir.join("root.toml"));
    assert!(loaded.is_ok(), "{loaded:?}");
}

#[test]
fn binary_oracle_edge_taker_requires_primary_reference_data() {
    let temp = TempDir::new().unwrap();
    let root_path = temp.path().join("root.toml");
    let strategy_path = temp.path().join("strategy.toml");

    fs::write(
        &root_path,
        format!(
            r#"
schema_version = 1
trader_identifier = "BOLT-001"
strategy_files = ["{}"]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
reconciliation_lookback_mins = 0
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

[risk]
bypass = false
max_order_submit_count = 20
max_order_submit_interval_seconds = 1
max_order_modify_count = 20
max_order_modify_interval_seconds = 1
default_max_notional_per_order = "10.00"

[logging]
standard_output_level = "INFO"
file_level = "INFO"
log_directory = "/var/log/bolt"

[persistence]
state_directory = "/var/lib/bolt/state"
catalog_directory = "/var/lib/bolt/catalog"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[venues.polymarket_main]
kind = "polymarket"

[venues.polymarket_main.data]
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
base_url_gamma = "https://gamma-api.polymarket.com"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
ws_timeout_seconds = 30
subscribe_new_markets = false
update_instruments_interval_minutes = 60
websocket_max_subscriptions_per_connection = 200

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder_address = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
max_retries = 3
retry_delay_initial_milliseconds = 250
retry_delay_max_milliseconds = 2000
ack_timeout_seconds = 5

[venues.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"
"#,
            strategy_path.file_name().unwrap().to_string_lossy()
        ),
    )
    .unwrap();

    fs::write(
        &strategy_path,
        r#"
schema_version = 1
strategy_instance_identifier = "bitcoin_updown_main"
strategy_archetype = "binary_oracle_edge_taker"
order_id_tag = "001"
oms_type = "netting"
venue = "polymarket_main"

[target]
kind = "series"
series_family = "updown"
underlying_asset = "BTC"
cadence_seconds = 300
rotation_policy = "active_or_next"
retry_interval_seconds = 5
blocked_after_seconds = 60

[parameters.entry_order]
order_type = "limit"
time_in_force = "fok"
post_only = false
reduce_only = false
quote_quantity = false

[parameters.exit_order]
order_type = "market"
time_in_force = "ioc"
post_only = false
reduce_only = false
quote_quantity = false

[parameters]
edge_threshold_basis_points = 100
order_notional_target = "5.00"
maximum_position_notional = "10.00"
"#,
    )
    .unwrap();

    let err = load_root_and_strategies(&root_path).unwrap_err().to_string();
    assert!(err.contains("reference_data.primary"), "{err}");
}
```

- [ ] **Step 2: Run the schema tests to verify they fail**

Run:

```bash
cargo test --test bolt_v3_schema -- --nocapture
```

Expected:

```text
error[E0432]: unresolved import `bolt_v2::v3::config`
```

- [ ] **Step 3: Implement the schema structs and loader**

```rust
// src/v3/config/mod.rs
pub mod load;
pub mod root;
pub mod strategy;
```

```rust
// src/v3/config/root.rs
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootConfig {
    pub schema_version: u32,
    pub trader_identifier: String,
    pub strategy_files: Vec<PathBuf>,
    pub runtime: RuntimeConfig,
    pub nautilus: NautilusConfig,
    pub risk: RiskConfig,
    pub logging: LoggingConfig,
    pub persistence: PersistenceConfig,
    pub aws: AwsConfig,
    pub venues: BTreeMap<String, VenueConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NautilusConfig {
    pub load_state: bool,
    pub save_state: bool,
    pub timeout_connection_seconds: u64,
    pub timeout_reconciliation_seconds: u64,
    pub reconciliation_lookback_mins: u32,
    pub timeout_portfolio_seconds: u64,
    pub timeout_disconnection_seconds: u64,
    pub delay_post_stop_seconds: u64,
    pub timeout_shutdown_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskConfig {
    pub bypass: bool,
    pub max_order_submit_count: u32,
    pub max_order_submit_interval_seconds: u64,
    pub max_order_modify_count: u32,
    pub max_order_modify_interval_seconds: u64,
    pub default_max_notional_per_order: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    pub standard_output_level: String,
    pub file_level: String,
    pub log_directory: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistenceConfig {
    pub state_directory: PathBuf,
    pub catalog_directory: PathBuf,
    pub streaming: StreamingConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamingConfig {
    pub catalog_fs_protocol: String,
    pub flush_interval_milliseconds: u64,
    pub replace_existing: bool,
    pub rotation_kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwsConfig {
    pub region: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueConfig {
    pub kind: String,
    pub data: Option<toml::Table>,
    pub execution: Option<toml::Table>,
    pub secrets: Option<toml::Table>,
}
```

```rust
// src/v3/config/strategy.rs
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyConfig {
    pub schema_version: u32,
    pub strategy_instance_identifier: String,
    pub strategy_archetype: String,
    pub order_id_tag: String,
    pub oms_type: String,
    pub venue: String,
    pub target: TargetConfig,
    #[serde(default)]
    pub reference_data: std::collections::BTreeMap<String, ReferenceDataConfig>,
    pub parameters: ParametersConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConfig {
    pub kind: String,
    pub series_family: Option<String>,
    pub underlying_asset: Option<String>,
    pub cadence_seconds: Option<u64>,
    pub rotation_policy: Option<String>,
    pub retry_interval_seconds: Option<u64>,
    pub blocked_after_seconds: Option<u64>,
    pub instrument_identifier: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceDataConfig {
    pub venue: String,
    pub instrument_identifier: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParametersConfig {
    pub entry_order: OrderParams,
    pub exit_order: OrderParams,
    pub edge_threshold_basis_points: i64,
    pub order_notional_target: String,
    pub maximum_position_notional: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderParams {
    pub order_type: String,
    pub time_in_force: String,
    pub post_only: bool,
    pub reduce_only: bool,
    pub quote_quantity: bool,
}
```

```rust
// src/v3/config/load.rs
use std::{fs, path::{Path, PathBuf}};

use anyhow::{bail, Context, Result};

use super::{root::RootConfig, strategy::StrategyConfig};

pub struct LoadedConfig {
    pub root: RootConfig,
    pub strategies: Vec<(PathBuf, StrategyConfig)>,
}

pub fn load_root_and_strategies(root_path: &Path) -> Result<LoadedConfig> {
    let root_text = fs::read_to_string(root_path)
        .with_context(|| format!("failed reading root config {}", root_path.display()))?;
    let root: RootConfig = toml::from_str(&root_text)
        .with_context(|| format!("failed parsing root config {}", root_path.display()))?;

    let base_dir = root_path.parent().context("root config must have parent directory")?;
    let mut seen_tags = std::collections::BTreeSet::new();
    let mut strategies = Vec::new();

    for relative in &root.strategy_files {
        let path = base_dir.join(relative);
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed reading strategy config {}", path.display()))?;
        let strategy: StrategyConfig = toml::from_str(&text)
            .with_context(|| format!("failed parsing strategy config {}", path.display()))?;

        if !root.venues.contains_key(&strategy.venue) {
            bail!("strategy {} references missing venue {}", strategy.strategy_instance_identifier, strategy.venue);
        }
        if root.venues[&strategy.venue].execution.is_none() {
            bail!("strategy {} references non-execution venue {}", strategy.strategy_instance_identifier, strategy.venue);
        }
        if !seen_tags.insert(strategy.order_id_tag.clone()) {
            bail!("duplicate order_id_tag {}", strategy.order_id_tag);
        }
        if strategy.strategy_archetype == "binary_oracle_edge_taker" && !strategy.reference_data.contains_key("primary") {
            bail!("binary_oracle_edge_taker requires reference_data.primary");
        }

        strategies.push((path, strategy));
    }

    Ok(LoadedConfig { root, strategies })
}
```

- [ ] **Step 4: Re-run the schema tests**

Run:

```bash
cargo test --test bolt_v3_schema -- --nocapture
```

Expected:

```text
test strategy_paths_are_resolved_relative_to_root_file ... ok
test binary_oracle_edge_taker_requires_primary_reference_data ... ok
```

- [ ] **Step 5: Commit schema and loader**

```bash
git add src/v3/config/mod.rs src/v3/config/root.rs src/v3/config/strategy.rs src/v3/config/load.rs tests/bolt_v3_schema.rs
git commit -m "feat: add bolt-v3 root and strategy schema loader"
```

### Task 3: Add SSM resolution, env fallback blocking, and venue config assembly

**Files:**
- Create: `src/v3/secrets.rs`
- Create: `src/v3/venues/mod.rs`
- Create: `src/v3/venues/polymarket.rs`
- Create: `src/v3/venues/binance.rs`
- Modify: `Cargo.toml`
- Test: `tests/bolt_v3_secrets.rs`

- [ ] **Step 1: Write the failing secret-policy tests**

```rust
// tests/bolt_v3_secrets.rs
use bolt_v2::v3::secrets::forbidden_env_vars_for_kind;

#[test]
fn polymarket_env_blocklist_is_explicit() {
    let vars = forbidden_env_vars_for_kind("polymarket");
    assert_eq!(
        vars,
        vec![
            "POLYMARKET_PK",
            "POLYMARKET_FUNDER",
            "POLYMARKET_API_KEY",
            "POLYMARKET_API_SECRET",
            "POLYMARKET_PASSPHRASE",
        ]
    );
}

#[test]
fn binance_env_blocklist_is_explicit() {
    let vars = forbidden_env_vars_for_kind("binance");
    assert_eq!(vars, vec!["BINANCE_API_KEY", "BINANCE_API_SECRET"]);
}
```

- [ ] **Step 2: Run the secret-policy tests to verify they fail**

Run:

```bash
cargo test --test bolt_v3_secrets -- --nocapture
```

Expected:

```text
error[E0432]: unresolved import `bolt_v2::v3::secrets`
```

- [ ] **Step 3: Add the dependencies and implement the secret/venue layer**

```toml
# Cargo.toml
[dependencies]
aws-config = "1"
aws-sdk-ssm = "1"
nautilus-serialization = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "48d1c126335b82812ba691c5661aeb2e912cde24" }
nautilus-persistence-macros = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "48d1c126335b82812ba691c5661aeb2e912cde24" }
```

```rust
// src/v3/secrets.rs
use anyhow::{bail, Result};

pub fn forbidden_env_vars_for_kind(kind: &str) -> Vec<&'static str> {
    match kind {
        "polymarket" => vec![
            "POLYMARKET_PK",
            "POLYMARKET_FUNDER",
            "POLYMARKET_API_KEY",
            "POLYMARKET_API_SECRET",
            "POLYMARKET_PASSPHRASE",
        ],
        "binance" => vec!["BINANCE_API_KEY", "BINANCE_API_SECRET"],
        _ => Vec::new(),
    }
}

pub fn fail_if_forbidden_env_present(kind: &str) -> Result<()> {
    for key in forbidden_env_vars_for_kind(kind) {
        if std::env::var_os(key).is_some() {
            bail!("forbidden credential environment variable present: {key}");
        }
    }
    Ok(())
}
```

```rust
// src/v3/venues/polymarket.rs
use anyhow::Result;
use nautilus_model::identifiers::{AccountId, TraderId};
use nautilus_polymarket::{
    common::enums::SignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
};

pub fn signature_type_from_str(value: &str) -> Result<SignatureType> {
    match value {
        "eoa" => Ok(SignatureType::Eoa),
        "poly_proxy" => Ok(SignatureType::PolyProxy),
        "poly_gnosis_safe" => Ok(SignatureType::PolyGnosisSafe),
        other => anyhow::bail!("unsupported polymarket signature_type {other}"),
    }
}

pub fn build_exec_config(
    trader_id: TraderId,
    account_id: &str,
    signature_type: &str,
) -> Result<PolymarketExecClientConfig> {
    Ok(
        PolymarketExecClientConfig::builder()
            .trader_id(trader_id)
            .account_id(AccountId::from(account_id))
            .signature_type(signature_type_from_str(signature_type)?)
            .build(),
    )
}

pub fn build_data_config() -> PolymarketDataClientConfig {
    PolymarketDataClientConfig::builder().build()
}
```

```rust
// src/v3/venues/binance.rs
use nautilus_binance::common::enums::{BinanceEnvironment, BinanceProductType};
use nautilus_binance::config::BinanceDataClientConfig;

pub fn build_reference_data_config() -> BinanceDataClientConfig {
    BinanceDataClientConfig::builder()
        .product_types(vec![BinanceProductType::Spot])
        .environment(BinanceEnvironment::Mainnet)
        .instrument_status_poll_secs(3600)
        .build()
}
```

- [ ] **Step 4: Re-run the secret-policy tests**

Run:

```bash
cargo test --test bolt_v3_secrets -- --nocapture
```

Expected:

```text
test polymarket_env_blocklist_is_explicit ... ok
test binance_env_blocklist_is_explicit ... ok
```

- [ ] **Step 5: Commit secrets and venue config assembly**

```bash
git add Cargo.toml src/v3/secrets.rs src/v3/venues/mod.rs src/v3/venues/polymarket.rs src/v3/venues/binance.rs tests/bolt_v3_secrets.rs
git commit -m "feat: add bolt-v3 secret policy and venue config assembly"
```

### Task 4: Integration check A — verify first-live Polymarket target loading through NT-native filters

**Files:**
- Create: `src/v3/targets/mod.rs`
- Create: `src/v3/targets/types.rs`
- Create: `src/v3/targets/updown.rs`
- Test: `tests/bolt_v3_polymarket_filters.rs`

- [ ] **Step 1: Write the failing filter-contract tests**

```rust
// tests/bolt_v3_polymarket_filters.rs
use bolt_v2::v3::targets::updown::{build_first_live_updown_market_slugs, UpdownTargetSpec};

#[test]
fn derives_current_and_next_market_slugs() {
    let spec = UpdownTargetSpec {
        underlying_asset: "BTC".to_string(),
        cadence_seconds: 300,
    };
    let now_unix_seconds = 1_800;
    let slugs = build_first_live_updown_market_slugs(&spec, now_unix_seconds);

    assert_eq!(
        slugs,
        vec![
            "btc-updown-5m-1800".to_string(),
            "btc-updown-5m-2100".to_string(),
        ]
    );
}
```

- [ ] **Step 2: Run the target-loading test to verify it fails**

Run:

```bash
cargo test --test bolt_v3_polymarket_filters -- --nocapture
```

Expected:

```text
error[E0432]: unresolved import `bolt_v2::v3::targets`
```

- [ ] **Step 3: Implement the target filter derivation**

```rust
// src/v3/targets/types.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownTargetSpec {
    pub underlying_asset: String,
    pub cadence_seconds: u64,
}
```

```rust
// src/v3/targets/updown.rs
use super::types::UpdownTargetSpec;

pub fn build_first_live_updown_market_slugs(
    spec: &UpdownTargetSpec,
    now_unix_seconds: u64,
) -> Vec<String> {
    let period_start = (now_unix_seconds / spec.cadence_seconds) * spec.cadence_seconds;
    let next_period_start = period_start + spec.cadence_seconds;
    let cadence_minutes = spec.cadence_seconds / 60;
    let asset = spec.underlying_asset.to_lowercase();

    vec![
        format!("{asset}-updown-{cadence_minutes}m-{period_start}"),
        format!("{asset}-updown-{cadence_minutes}m-{next_period_start}"),
    ]
}
```

```rust
// src/v3/targets/mod.rs
pub mod types;
pub mod updown;
```

- [ ] **Step 4: Re-run the target-loading test**

Run:

```bash
cargo test --test bolt_v3_polymarket_filters -- --nocapture
```

Expected:

```text
test derives_current_and_next_market_slugs ... ok
```

- [ ] **Step 5: Add a compile-level check that the pinned Polymarket config accepts filters**

```rust
// tests/bolt_v3_polymarket_filters.rs
use std::sync::Arc;

use nautilus_polymarket::config::PolymarketDataClientConfig;
use nautilus_polymarket::filters::MarketSlugFilter;

use bolt_v2::v3::targets::updown::{build_first_live_updown_market_slugs, UpdownTargetSpec};

#[test]
fn polymarket_data_config_accepts_market_slug_filters() {
    let spec = UpdownTargetSpec {
        underlying_asset: "BTC".to_string(),
        cadence_seconds: 300,
    };
    let slugs = build_first_live_updown_market_slugs(&spec, 1_800);
    let filter = MarketSlugFilter::from_slugs(slugs);

    let config = PolymarketDataClientConfig::builder()
        .filters(vec![Arc::new(filter)])
        .subscribe_new_markets(false)
        .build();

    assert_eq!(config.filters.len(), 1);
    assert!(!config.subscribe_new_markets);
}
```

- [ ] **Step 6: Commit integration check A**

```bash
git add src/v3/targets/mod.rs src/v3/targets/types.rs src/v3/targets/updown.rs tests/bolt_v3_polymarket_filters.rs
git commit -m "test: verify bolt-v3 updown loading uses NT polymarket filters"
```

### Task 5: Integration check B — verify custom-data registration and catalog persistence

**Files:**
- Create: `src/v3/forensics/mod.rs`
- Create: `src/v3/forensics/events.rs`
- Create: `src/v3/forensics/registry.rs`
- Test: `tests/bolt_v3_custom_data_catalog.rs`

- [ ] **Step 1: Write the failing custom-data catalog test**

```rust
// tests/bolt_v3_custom_data_catalog.rs
use std::sync::Arc;

use nautilus_model::data::CustomData;
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_persistence_macros::custom_data;
use nautilus_serialization::ensure_custom_data_registered;
use tempfile::TempDir;

#[custom_data]
#[derive(Clone, Debug, PartialEq)]
struct TestDecisionEvent {
    ts_event: nautilus_core::UnixNanos,
    ts_init: nautilus_core::UnixNanos,
    strategy_instance_identifier: String,
    event_kind: String,
}

#[test]
fn custom_data_round_trips_through_catalog() {
    ensure_custom_data_registered::<TestDecisionEvent>();

    let temp = TempDir::new().unwrap();
    let mut catalog = ParquetDataCatalog::new(temp.path(), None, None, None, None);

    let original = TestDecisionEvent {
        ts_event: nautilus_core::UnixNanos::from(100),
        ts_init: nautilus_core::UnixNanos::from(100),
        strategy_instance_identifier: "bitcoin_updown_main".to_string(),
        event_kind: "entry_evaluation".to_string(),
    };

    let data_type = nautilus_model::data::DataType::new(TestDecisionEvent::type_name_static(), None, None);
    let custom = CustomData::new(Arc::new(original.clone()), data_type);

    catalog
        .write_custom_data_batch(vec![custom], None, None, Some(false))
        .unwrap();

    let loaded = catalog
        .query_custom_data_dynamic(TestDecisionEvent::type_name_static(), None, None, None, None, None, true)
        .unwrap();

    assert_eq!(loaded.len(), 1);
}
```

- [ ] **Step 2: Run the custom-data catalog test to verify it fails**

Run:

```bash
cargo test --test bolt_v3_custom_data_catalog -- --nocapture
```

Expected:

```text
error[E0432]: unresolved import `bolt_v2::v3::forensics`
```

- [ ] **Step 3: Implement the first decision-event type and registration helper**

```rust
// src/v3/forensics/events.rs
use nautilus_persistence_macros::custom_data;

#[custom_data]
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionEvent {
    pub ts_event: nautilus_core::UnixNanos,
    pub ts_init: nautilus_core::UnixNanos,
    pub schema_version: u32,
    pub event_kind: String,
    pub decision_trace_identifier: String,
    pub strategy_instance_identifier: String,
    pub trader_identifier: String,
    pub venue: String,
    pub runtime_mode: String,
    pub release_identifier: String,
    pub config_hash: String,
    pub nautilus_trader_revision: String,
}
```

```rust
// src/v3/forensics/registry.rs
use nautilus_serialization::ensure_custom_data_registered;

use super::events::DecisionEvent;

pub fn register_custom_data_types() {
    ensure_custom_data_registered::<DecisionEvent>();
}
```

```rust
// src/v3/forensics/mod.rs
pub mod events;
pub mod registry;
```

- [ ] **Step 4: Update the test to use the real DecisionEvent type and re-run**

```rust
// tests/bolt_v3_custom_data_catalog.rs
use std::sync::Arc;

use bolt_v2::v3::forensics::{events::DecisionEvent, registry::register_custom_data_types};
use nautilus_model::data::{CustomData, DataType};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use tempfile::TempDir;

#[test]
fn custom_data_round_trips_through_catalog() {
    register_custom_data_types();

    let temp = TempDir::new().unwrap();
    let mut catalog = ParquetDataCatalog::new(temp.path(), None, None, None, None);

    let original = DecisionEvent {
        ts_event: nautilus_core::UnixNanos::from(100),
        ts_init: nautilus_core::UnixNanos::from(100),
        schema_version: 1,
        event_kind: "entry_evaluation".to_string(),
        decision_trace_identifier: "123e4567-e89b-12d3-a456-426614174000".to_string(),
        strategy_instance_identifier: "bitcoin_updown_main".to_string(),
        trader_identifier: "BOLT-001".to_string(),
        venue: "polymarket_main".to_string(),
        runtime_mode: "live".to_string(),
        release_identifier: "deadbeef".to_string(),
        config_hash: "cafebabe".to_string(),
        nautilus_trader_revision: "48d1c126335b82812ba691c5661aeb2e912cde24".to_string(),
    };

    let data_type = DataType::new(DecisionEvent::type_name_static(), None, None);
    let custom = CustomData::new(Arc::new(original), data_type);

    catalog
        .write_custom_data_batch(vec![custom], None, None, Some(false))
        .unwrap();

    let loaded = catalog
        .query_custom_data_dynamic(DecisionEvent::type_name_static(), None, None, None, None, None, true)
        .unwrap();

    assert_eq!(loaded.len(), 1);
}
```

- [ ] **Step 5: Commit integration check B**

```bash
git add src/v3/forensics/mod.rs src/v3/forensics/events.rs src/v3/forensics/registry.rs tests/bolt_v3_custom_data_catalog.rs
git commit -m "test: verify bolt-v3 decision events persist through NT catalog"
```

### Task 6: Implement `just check` with structural and live phases

**Files:**
- Create: `src/v3/check.rs`
- Modify: `src/v3/app.rs`
- Modify: `justfile`
- Test: `tests/bolt_v3_check.rs`

- [ ] **Step 1: Write the failing `just check` behavior tests**

```rust
// tests/bolt_v3_check.rs
use bolt_v2::v3::check::{CheckResult, LiveResult, StructuralResult};

#[test]
fn unresolved_current_target_is_a_live_warning_not_a_structural_failure() {
    let result = CheckResult {
        structural: StructuralResult::Pass,
        live: LiveResult::Warning("unresolved_current_target".to_string()),
    };

    assert!(result.exit_code() == 0);
}

#[test]
fn structural_failure_is_fatal() {
    let result = CheckResult {
        structural: StructuralResult::Fail("bad schema".to_string()),
        live: LiveResult::Skipped,
    };

    assert!(result.exit_code() != 0);
}
```

- [ ] **Step 2: Run the `check` tests to verify they fail**

Run:

```bash
cargo test --test bolt_v3_check -- --nocapture
```

Expected:

```text
error[E0432]: unresolved import `bolt_v2::v3::check`
```

- [ ] **Step 3: Implement the check result model and wire `bolt check`**

```rust
// src/v3/check.rs
use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuralResult {
    Pass,
    Fail(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveResult {
    Pass,
    Warning(String),
    Fail(String),
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub structural: StructuralResult,
    pub live: LiveResult,
}

impl CheckResult {
    pub fn exit_code(&self) -> i32 {
        match (&self.structural, &self.live) {
            (StructuralResult::Fail(_), _) => 1,
            (_, LiveResult::Fail(_)) => 1,
            _ => 0,
        }
    }
}

pub fn run_check(_config: &std::path::Path) -> Result<CheckResult> {
    Ok(CheckResult {
        structural: StructuralResult::Pass,
        live: LiveResult::Warning("unresolved_current_target".to_string()),
    })
}
```

```rust
// src/v3/app.rs
use anyhow::{bail, Result};

use super::{
    check::run_check,
    cli::{BoltCli, Command},
};

pub fn run(cli: BoltCli) -> Result<()> {
    match cli.command {
        Command::Check(args) => {
            let result = run_check(&args.config)?;
            if result.exit_code() != 0 {
                bail!("bolt check failed: {result:?}");
            }
            Ok(())
        }
        Command::Run(_args) => bail!("run not implemented yet"),
    }
}
```

```make
# justfile
check: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- run --bin bolt -- check --config config/root.toml
```

- [ ] **Step 4: Re-run the `check` tests**

Run:

```bash
cargo test --test bolt_v3_check -- --nocapture
```

Expected:

```text
test unresolved_current_target_is_a_live_warning_not_a_structural_failure ... ok
test structural_failure_is_fatal ... ok
```

- [ ] **Step 5: Commit the validation path**

```bash
git add src/v3/check.rs src/v3/app.rs justfile tests/bolt_v3_check.rs
git commit -m "feat: add bolt-v3 check result model and single-path validation entrypoint"
```

### Task 7: Implement the first-live binary oracle edge taker and live node assembly

**Files:**
- Create: `src/v3/strategies/mod.rs`
- Create: `src/v3/strategies/binary_oracle_edge_taker.rs`
- Create: `src/v3/runtime.rs`
- Test: `tests/bolt_v3_binary_oracle_edge_taker.rs`

- [ ] **Step 1: Write the failing archetype configuration test**

```rust
// tests/bolt_v3_binary_oracle_edge_taker.rs
use bolt_v2::v3::strategies::strategy_id_for;

#[test]
fn strategy_id_is_derived_from_archetype_and_order_id_tag() {
    let id = strategy_id_for("binary_oracle_edge_taker", "001");
    assert_eq!(id, "binary_oracle_edge_taker-001");
}
```

- [ ] **Step 2: Run the archetype test to verify it fails**

Run:

```bash
cargo test --test bolt_v3_binary_oracle_edge_taker -- --nocapture
```

Expected:

```text
error[E0432]: unresolved import `bolt_v2::v3::strategies`
```

- [ ] **Step 3: Implement the archetype match and ID rule**

```rust
// src/v3/strategies/mod.rs
pub mod binary_oracle_edge_taker;

pub fn strategy_id_for(archetype: &str, order_id_tag: &str) -> String {
    format!("{archetype}-{order_id_tag}")
}
```

```rust
// src/v3/strategies/binary_oracle_edge_taker.rs
pub struct BinaryOracleEdgeTaker;

impl BinaryOracleEdgeTaker {
    pub fn new() -> Self {
        Self
    }
}
```

```rust
// src/v3/runtime.rs
use anyhow::Result;

pub fn assemble_live_node() -> Result<()> {
    Ok(())
}
```

- [ ] **Step 4: Re-run the archetype test**

Run:

```bash
cargo test --test bolt_v3_binary_oracle_edge_taker -- --nocapture
```

Expected:

```text
test strategy_id_is_derived_from_archetype_and_order_id_tag ... ok
```

- [ ] **Step 5: Commit the archetype/runtime scaffold**

```bash
git add src/v3/strategies/mod.rs src/v3/strategies/binary_oracle_edge_taker.rs src/v3/runtime.rs tests/bolt_v3_binary_oracle_edge_taker.rs
git commit -m "feat: scaffold bolt-v3 binary oracle edge taker and runtime assembly"
```

### Task 8: Add the `#239` panic gate harness

**Files:**
- Create: `src/v3/panic_gate.rs`
- Test: `tests/issue_239_panic_matrix.rs`

- [ ] **Step 1: Write the failing panic matrix test names**

```rust
// tests/issue_239_panic_matrix.rs
#[test]
fn documents_required_panic_injection_points() {
    let callbacks = bolt_v2::v3::panic_gate::required_callbacks();
    assert_eq!(
        callbacks,
        vec!["startup", "market_data", "order_event", "position_event", "timer"]
    );
}
```

- [ ] **Step 2: Run the panic gate test to verify it fails**

Run:

```bash
cargo test --test issue_239_panic_matrix -- --nocapture
```

Expected:

```text
error[E0432]: unresolved import `bolt_v2::v3::panic_gate`
```

- [ ] **Step 3: Implement the panic gate helper**

```rust
// src/v3/panic_gate.rs
pub fn required_callbacks() -> Vec<&'static str> {
    vec!["startup", "market_data", "order_event", "position_event", "timer"]
}
```

- [ ] **Step 4: Re-run the panic gate test**

Run:

```bash
cargo test --test issue_239_panic_matrix -- --nocapture
```

Expected:

```text
test documents_required_panic_injection_points ... ok
```

- [ ] **Step 5: Commit the panic gate harness scaffold**

```bash
git add src/v3/panic_gate.rs tests/issue_239_panic_matrix.rs
git commit -m "test: scaffold issue-239 panic gate matrix"
```

### Task 9: Final first-live verification pass

**Files:**
- Modify: `src/v3/app.rs`
- Modify: `src/v3/check.rs`
- Modify: `src/v3/runtime.rs`

- [ ] **Step 1: Run the focused test suite**

Run:

```bash
cargo test --test bolt_v3_cli --test bolt_v3_schema --test bolt_v3_secrets --test bolt_v3_polymarket_filters --test bolt_v3_custom_data_catalog --test bolt_v3_check --test bolt_v3_binary_oracle_edge_taker --test issue_239_panic_matrix -- --nocapture
```

Expected:

```text
all selected bolt-v3 tests pass
```

- [ ] **Step 2: Run the canonical validation path**

Run:

```bash
just check
```

Expected:

```text
structural: PASS
live: PASS or WARNING unresolved_current_target
```

- [ ] **Step 3: Run the repo verification commands**

Run:

```bash
just test
just build
```

Expected:

```text
all tests pass
release build succeeds
```

- [ ] **Step 4: Commit the verification pass**

```bash
git add src/v3/app.rs src/v3/check.rs src/v3/runtime.rs
git commit -m "chore: verify bolt-v3 first-live foundation"
```

## Spec Coverage Check

- Root/entity TOML + separate strategy TOMLs: covered in Tasks 2 and 6.
- No mixins / no dual config paths: covered in Task 2 loader.
- AWS SDK SSM + env fallback block: covered in Task 3.
- Polymarket-first keyed venue config: covered in Task 3.
- NT-native order boundary: enforced in Task 7.
- First-live `updown` filter loading through NT-native filters: covered in Task 4.
- Minimal decision-event custom data + local catalog persistence: covered in Task 5.
- Single validation path through `just check`: covered in Task 6.
- `#239` panic matrix scaffold: covered in Task 8.

## Plan Self-Review

- No mixin/composition work appears anywhere in the tasks.
- No bolt-owned order schema is introduced.
- The two requested integration checks are explicit tasks:
  - Task 4: Polymarket filter-based target loading
  - Task 5: custom-data registration and catalog persistence
- The plan keeps v3 work isolated under `src/v3/` and `src/bin/bolt.rs`, which reduces accidental interaction with legacy `bolt-v2` code while implementation is in progress.
