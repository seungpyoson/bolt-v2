# OKX Required Config Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the OKX raw-key guard ladder with one typed, required configuration parser used by both validation and adapter mapping.

**Architecture:** The shared data-only provider path receives required parser and validator functions. Ordinary providers use the generic Nautilus deserializer; OKX uses a typed monitor-control parser that constructs one official `OKXDataClientConfig` with the explicitly configured values. No optional parser/validator dispatch, field-list scan, fallback value, or second OKX-only mapping guard remains.

**Tech Stack:** Rust 1.97.0, serde, toml, NautilusTrader 0.61.0, GitHub Actions remote Rust verification.

## Global Constraints

- Stay within issue #1383's official-source correction slice.
- No conditional fallback, alternate provider/value source, or named-field guard ladder.
- Preserve `book_stale_check_interval_secs = 0`, `book_stale_threshold_secs = 0`, and `book_snapshot_timeout_secs = 3` from TOML exactly.
- Use evidence-driven verification; do not run compile-heavy Rust checks locally.
- Do not request review until the exact pushed head is green and internally reviewed.

---

### Task 1: Make the data-config boundary uniformly typed

**Files:**
- Modify: `src/bolt_v3_providers/market_data.rs`
- Test: `tests/bolt_v3_provider_binding.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_maker/mod.rs`
- Modify: `src/strategies/complete_set_arbitrage/mod.rs`

**Interfaces:**
- Consumes: `toml::Value`, official `OKXDataClientConfig`, existing `validate_data_only_client` and `map_data_only_adapters` entry points.
- Produces: `parse_okx_data_config(&toml::Value) -> Result<OKXDataClientConfig, String>` and required `DataConfigParser<T>` / `DataConfigValidator<T>` arguments shared by validation and mapping.

- [ ] **Step 1: Replace procedural OKX key checks with typed controls**

Add the typed input and the one parser that constructs the official config:

```rust
#[derive(Debug, Deserialize)]
struct RequiredOkxBookHealthControls {
    book_stale_check_interval_secs: u64,
    book_stale_threshold_secs: u64,
    book_snapshot_timeout_secs: u64,
}

fn deserialize_data_config<T>(value: &toml::Value) -> Result<T, String>
where
    T: DeserializeOwned,
{
    value.clone().try_into().map_err(|error: toml::de::Error| error.to_string())
}

fn parse_okx_data_config(value: &toml::Value) -> Result<OKXDataClientConfig, String> {
    let controls = deserialize_data_config::<RequiredOkxBookHealthControls>(value)?;
    let mut config = deserialize_data_config::<OKXDataClientConfig>(value)?;
    config.book_stale_check_interval_secs = controls.book_stale_check_interval_secs;
    config.book_stale_threshold_secs = controls.book_stale_threshold_secs;
    config.book_snapshot_timeout_secs = controls.book_snapshot_timeout_secs;
    Ok(config)
}
```

Delete `OKX_REQUIRED_DATA_FIELDS`, `missing_data_fields`, `reject_missing_data_fields_for_mapping`, and both OKX-only raw-table guard blocks.

- [ ] **Step 2: Require parser and validator functions on the shared path**

Change both generic boundaries to take required functions:

```rust
type DataConfigParser<T> = fn(&toml::Value) -> Result<T, String>;
type DataConfigValidator<T> = fn(&T) -> anyhow::Result<()>;

fn accept_data_config<T>(_config: &T) -> anyhow::Result<()> {
    Ok(())
}
```

`validate_data_only_client` always calls its parser and validator. Every direct provider binding passes `deserialize_data_config::<T>` plus `accept_data_config::<T>`, except OKX passes `parse_okx_data_config` and Kraken passes `KrakenDataClientConfig::validate`. `map_data_only_adapters` always uses its required parser function to build the exact official config trait object.

- [ ] **Step 3: Preserve fail-closed tests through the typed boundary**

Rename
`okx_monitor_compatibility_fields_are_required_at_validation_and_mapping_boundaries`
to `okx_monitor_controls_are_required_by_typed_config_boundary`. Retain its
existing table-driven field removal and both validation/mapping error
assertions without adding provider-specific branches.

Retain the existing positive downcast assertions proving the official config contains `0`, `0`, and `3` exactly.

- [ ] **Step 4: Fix the exact remote compiler diagnostics**

Remove the now-unused `component::Component` imports from the three strategy modules. Do not add `allow` attributes; the remote failure must be eliminated at its source.

- [ ] **Step 5: Format and inspect the implementation**

Run:

```bash
cargo fmt
git diff --check
rg -n 'OKX_REQUIRED_DATA_FIELDS|missing_data_fields|reject_missing_data_fields_for_mapping|compatibility_fields' src tests
```

Expected: formatting and diff checks pass; the final search has no matches.

- [ ] **Step 6: Commit the implementation**

```bash
git add src/bolt_v3_providers/market_data.rs tests/bolt_v3_provider_binding.rs \
  src/strategies/binary_oracle_edge_taker/mod.rs \
  src/strategies/binary_oracle_maker/mod.rs \
  src/strategies/complete_set_arbitrage/mod.rs
git commit -m "fix: require typed okx monitor config"
```

### Task 2: Prove the single path and publish exact-head evidence

**Files:**
- Verify: `src/bolt_v3_providers/market_data.rs`
- Verify: `tests/bolt_v3_provider_binding.rs`
- Verify: all files changed by PR #1447

**Interfaces:**
- Consumes: committed Task 1 implementation.
- Produces: a clean pushed head with local non-compile evidence, focused remote Rust evidence if suggested, full exact-head CI, and a final internal adversarial review.

- [ ] **Step 1: Run local non-compile evidence**

Run:

```bash
just fmt-check
just bte-fmt-check
just deny
just ci-lint-workflow
just source-fence-static
```

Expected: all commands pass. Also rerun the focused Python verifier suites changed by this PR and require all tests to pass.

- [ ] **Step 2: Publish through the governed path**

Run `just sandbox-safe-push`, then verify the remote branch SHA equals local `HEAD`. Do not request review.

- [ ] **Step 3: Obtain the smallest remote Rust feedback**

Run `just rust-probe suggest`. If it recommends the `bolt_v3_provider_binding` integration target or a focused library/clippy probe, dispatch only the smallest sufficient probe from the clean pushed head. Stop after two failed probes and diagnose before spending another run.

- [ ] **Step 4: Run exact-head full verification**

Run `just verify-remote` once the slice is coherent. Require `actionlint`, `gate`, `backtester-gate`, and `host-health` to be green for the exact head under the pre-cutover rules.

- [ ] **Step 5: Conduct the final internal adversarial review**

Inspect `base...HEAD` and prove:

- no added runtime `.or`, `.or_else`, fallback provider, compatibility adapter, or named-field guard ladder;
- one OKX parser is used by validation and mapping;
- the official config receives only the three required typed TOML values;
- all seven catalog projections validate every row identity;
- every catalog Money path uses exact decimal parsing;
- capability evidence remains generic and manifest-driven;
- the worktree is clean and local, remote, and PR head SHAs agree.

- [ ] **Step 6: Request the required human review**

Only after every prior step succeeds, inspect and resolve applicable review threads, generate the immutable-boundary final review prompt, and request review from the current login for node ID `U_kgDOEZMFhA`. Do not merge.
