# NT CLOB V2 Pin Compatibility Probe

Date: 2026-04-28

## Executive Result

This probe moved the local compatibility worktree from Bolt's current NT pin
`48d1c126335b82812ba691c5661aeb2e912cde24` to candidate pin
`56a438216442f079edf322a39cdc0d9e655ba6d8` and applied only the minimum compile
fixes needed for focused Bolt-v3 and adjacent factory-support tests.

The focused probe passed. This is not production readiness, not a CLOB V2
closure claim, and not live-trading validation. It only shows that the audited
pin can compile through the checked Bolt-v3 surfaces after the narrow API fixes
listed below.

## Probe Branch

- Base branch: `feat/236-bolt-v3-slice-8-1-baseline`
- Base commit: `b290678`
- Probe branch: `probe/nt-clob-v2-compat-b290678`
- Probe worktree: `.worktrees/probe-nt-clob-v2-compat-b290678`
- Stale worktree not reused: `.worktrees/probe-nt-polymarket-v2-compat`

## Pin And Toolchain Changes

- `Cargo.toml`: all direct NautilusTrader git dependencies now pin
  `56a438216442f079edf322a39cdc0d9e655ba6d8`.
- `Cargo.lock`: updated with `/Users/spson/.cargo/bin/cargo update`; Cargo
  reported 114 locked package changes, including NT crates moving to `0.56.0`.
- `rust-version`: raised from `1.94.1` to `1.95.0`.
- `rust-toolchain.toml`: channel raised from `1.94.1` to `1.95.0`.
- `aws-sdk-ssm`: narrowed to `default-features = false` with
  `default-https-client` and `rt-tokio` enabled. This removes the legacy
  AWS Hyper 0.14 / rustls 0.21 TLS path while preserving the SSM client path
  Bolt uses for secrets.

## Actual Compile Fallout

1. `nautilus_system::factories` moved to `nautilus_common::factories`.
   Affected Bolt import sites:
   - `src/clients/mod.rs`
   - `src/live_node_setup.rs`
   - `src/clients/chainlink.rs`
   - `src/platform/runtime.rs`
   - `tests/support/mod.rs`
   - `tests/platform_runtime.rs`
   - `tests/reference_pipeline.rs`

2. Polymarket config literals gained required fields.
   - `PolymarketDataClientConfig` now requires
     `auto_load_missing_instruments`, `auto_load_debounce_ms`, and
     `transport_backend`.
   - `PolymarketExecClientConfig` now requires `transport_backend`.
   - Bolt sets `auto_load_missing_instruments: false` to preserve the
     controlled-loading contract in the Bolt-v3 adapter path.
   - `auto_load_debounce_ms: 100` is hardcoded in this probe and is only
     effective when auto-load is enabled, which is false in the Bolt-v3 path.

   `src/clients/polymarket.rs` compiled without source changes because its
   existing `..Default::default()` absorbed the new fields. That leaves the
   legacy/shared path inheriting NT's candidate default
   `auto_load_missing_instruments = true` at runtime. This probe does not
   validate that path for live use.

3. Binance data config literals gained `transport_backend`.
   - `src/bolt_v3_adapters.rs`
   - `src/clients/binance.rs`

4. `DataClient` subscription methods now take owned command values in the
   surfaces Bolt implements in tests and Chainlink.
   - `src/clients/chainlink.rs`
   - `tests/support/mod.rs`

5. `ExecutionClient::submit_order` now takes an owned `SubmitOrder`.
   - `tests/support/mod.rs`

6. `WebSocketConfig` gained `proxy_url` and `backend`.
   - `src/raw_capture_transport.rs`

7. `GammaEvent` gained `game_id`.
   - `src/clients/polymarket.rs` test literals set `game_id: None`.

## Verification Run

All commands were run in `.worktrees/probe-nt-clob-v2-compat-b290678` with
`/Users/spson/.cargo/bin/cargo` to use Rust/Cargo 1.95.0 directly.

- `/Users/spson/.cargo/bin/cargo test --test bolt_v3_adapter_mapping`:
  passed, 7 tests.
- `/Users/spson/.cargo/bin/cargo test --test bolt_v3_client_registration`:
  passed, 5 tests.
- `/Users/spson/.cargo/bin/cargo test --test bolt_v3_provider_binding`:
  passed, 7 tests.
- `/Users/spson/.cargo/bin/cargo test --test bolt_v3_credential_log_suppression`:
  passed, 1 test.
- `/Users/spson/.cargo/bin/cargo test --test live_node_run`:
  passed, 5 tests.
- `/Users/spson/.cargo/bin/cargo test --test bolt_v3_controlled_connect`:
  passed, 8 tests.
- `/Users/spson/.cargo/bin/cargo test --test reference_pipeline`:
  passed, 15 tests.
- `/Users/spson/.cargo/bin/cargo test --test platform_runtime`:
  passed, 20 tests.

## Residual Risk

- This probe did not validate live Polymarket CLOB V2 order submission, fills,
  signing, or match-time fee behavior.
- The legacy/shared Polymarket path in `src/clients/polymarket.rs` was not
  changed. A production pin-bump slice must either explicitly set
  `auto_load_missing_instruments = false` there or remove/deprecate that path.
- `src/clients/polymarket.rs` also inherits the new
  `PolymarketExecClientConfig.transport_backend` default through
  `..Default::default()`.
- `transport_backend: Default::default()` and `WebSocketConfig.backend:
  Default::default()` resolve to NT candidate defaults; this probe did not
  characterise the selected runtime transport implementations.
- Inherited live-engine defaults were not explicitly reviewed for production
  behavior. Candidate NT changes include
  `LiveExecEngineConfig.max_single_order_queries_per_cycle = 10` and
  `LiveExecEngineConfig.position_check_threshold_ms = 5_000`.
- This probe did not validate Binance runtime behavior; Binance changes were
  compile/config compatibility only.
- No `BinanceExecClientConfig` struct literal exists in Bolt in this probe, so
  candidate fields such as `use_trade_lite` are not exercised here.
- This probe did not refactor provider boundaries or update runtime contracts.
- `Cargo.lock` churn is material and should be reviewed separately before any
  production pin bump, including network/crypto/transitive dependency changes.
- NT runtime logs report `nautilus_trader: 1.226.0`; the Rust crate workspace
  versions moved to `0.56.0`.

## Production Pin-Bump Gates

Before any real pin-bump PR, run broader verification and dependency review:

- `/Users/spson/.cargo/bin/cargo test --workspace --all-targets`
- `/Users/spson/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings`
- `/Users/spson/.cargo/bin/cargo deny check`
- `cargo audit`, if available
- `cargo tree -d`
- A source-grounded decision on whether the legacy/shared Polymarket path remains
  callable
- Runtime-contract updates for any accepted CLOB V2, auto-load, transport, and
  live-engine-default decisions

## Review Follow-Up

After PR review, the legacy/shared Polymarket path decision was tracked in
GitHub issue #261. That issue must be resolved before live use or before the
legacy path is treated as production-valid under this NT pin.

Additional dependency checks run after the initial probe:

- `/Users/spson/.cargo/bin/cargo tree -d --depth 0`: completed and reported
  duplicate dependency families including `rustls` `0.21.12` / `0.23.39`,
  `rustls-webpki` `0.101.7` / `0.103.13`, `hyper-rustls` `0.24.2` / `0.27.9`,
  and other expected lockfile churn from the NT/AWS/network stack.
- `/Users/spson/.cargo/bin/cargo deny check`: failed on
  `rustls-webpki 0.101.7` advisories `RUSTSEC-2026-0098`,
  `RUSTSEC-2026-0099`, and `RUSTSEC-2026-0104`, reached through
  `aws-smithy-http-client` / `rustls 0.21.12`.
- `/Users/spson/.cargo/bin/cargo update -p rustls-webpki@0.101.7` and a
  targeted AWS SDK stack update did not move the vulnerable transitive version
  within the existing constraints.
- The dependency-security gate was then fixed by disabling default features on
  the direct `aws-sdk-ssm` dependency and enabling only `default-https-client`
  and `rt-tokio`. Cargo removed `hyper 0.14.32`, `hyper-rustls 0.24.2`,
  `rustls 0.21.12`, `rustls-webpki 0.101.7`, `tokio-rustls 0.24.1`, and
  related legacy TLS crates from `Cargo.lock`.
- `/Users/spson/.cargo/bin/cargo tree -i rustls-webpki@0.101.7`: no matching
  package remains in the resolved graph.
- `/Users/spson/.cargo/bin/cargo tree -i rustls@0.21.12`: no matching package
  remains in the resolved graph.
- `/Users/spson/.cargo/bin/cargo tree -d --depth 0`: completed after the fix
  without the old `rustls` `0.21.12`, `rustls-webpki` `0.101.7`, or
  `hyper-rustls` `0.24.2` duplicate families.
- `/Users/spson/.cargo/bin/cargo deny check`: passed after the AWS SSM feature
  narrowing (`advisories ok, bans ok, licenses ok, sources ok`).
- `/Users/spson/.cargo/bin/cargo audit`: unavailable in this environment
  (`cargo` reports no `audit` subcommand).

This means PR #260 has dependency-security evidence for the previously failing
`cargo deny check`. It remains a compatibility probe, not a CLOB V2 live-readiness
claim.

## Explicit Non-Goals

- No merge.
- No push.
- No live trading.
- No provider-boundary refactor.
- No feature work.
- No old Bolt v1 inspection.
- No secrets or `.env` inspection.
