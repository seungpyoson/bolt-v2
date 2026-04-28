# NautilusTrader CLOB V2 Pin-Change Audit

Date: 2026-04-28

Scope: audit only. No Cargo pin bump, lockfile update, merge, push, or live-trading change was performed.

## 1. Executive Verdict

Moving Bolt from NautilusTrader `48d1c126335b82812ba691c5661aeb2e912cde24` to candidate `56a438216442f079edf322a39cdc0d9e655ba6d8` would reduce the current Polymarket CLOB V2 signing blocker, because the candidate commit directly migrates NT's Polymarket adapter to CLOB V2.

This does not solve CLOB V2 for Bolt. Bolt must still pin the candidate in a separate probe, compile, test, and update the runtime contracts/status evidence before the blocker can be closed.

It is not a safe one-line pin bump. The candidate is on upstream `develop`, not on `master` or a tag, and the diff from current pin to candidate includes broad NT API drift:

- `nautilus_system::factories::{ClientConfig, DataClientFactory, ExecutionClientFactory}` moved to `nautilus_common::factories`.
- `PolymarketDataClientConfig` gained `auto_load_missing_instruments`, `auto_load_debounce_ms`, and `transport_backend`.
- `PolymarketExecClientConfig` gained `transport_backend`.
- `BinanceDataClientConfig` and `BinanceExecClientConfig` also gained `transport_backend`.
- `LoggerConfig` gained fields, though Bolt's current `..Default::default()` construction should absorb this.
- `FillReport` gained `avg_px`, mostly harmless for Bolt unless direct struct literals are introduced.

Recommended next slice: request adversarial review of this audit before any probe. If the audit passes review, create a separate compatibility probe branch/worktree from `feat/236-bolt-v3-slice-8-1-baseline` with only the NT pin bump plus minimum compile fixes. Do not use `main` as the probe base because `main` is missing the slice-8-1 / market-identity / source-grounded-status-map work represented by this branch. Do not include provider-boundary refactor, feature work, or live trading behavior in the probe. Use that probe only to quantify exact compile/test fallout before deciding whether any production pin-bump slice is warranted.

## 2. Current Pin vs Candidate Pin

Current Bolt pin:

- `Cargo.toml` pins all NT crates, including `nautilus-polymarket`, to `48d1c126335b82812ba691c5661aeb2e912cde24`.
- `Cargo.lock` resolves NT crates from `git+https://github.com/nautechsystems/nautilus_trader.git?rev=48d1c126335b82812ba691c5661aeb2e912cde24#48d1c126335b82812ba691c5661aeb2e912cde24`.
- Upstream `refs/tags/v1.225.0^{}` and `refs/heads/master` both point at the current pin.

Candidate:

- Commit: `56a438216442f079edf322a39cdc0d9e655ba6d8`
- Subject: `Migrate Polymarket adapter to CLOB V2`
- Author date: `2026-04-28T20:02:53+10:00`
- Present on upstream branches: `origin/develop`, `origin/test-linux-arm-publish`, `origin/test-macos-publish`.
- No upstream tag contains it; `git describe --contains 56a438216...` failed.
- `origin/master` does not contain it.

Pinning directly to the commit SHA is reproducible. Pinning to a branch is not.

Evidence commands used:

- `rg -n "nautilus|48d1c126|56a438216|rev =" Cargo.toml Cargo.lock`
- `git ls-remote --heads https://github.com/nautechsystems/nautilus_trader.git`
- `git -C /tmp/nt-pin-change-audit-nautilus-full-20260428 branch -r --contains 56a438216442f079edf322a39cdc0d9e655ba6d8`
- `git -C /tmp/nt-pin-change-audit-nautilus-full-20260428 tag --contains 56a438216442f079edf322a39cdc0d9e655ba6d8`

## 3. Upstream Polymarket CLOB V2 Changes

Primary upstream files inspected:

- `crates/adapters/polymarket/src/signing/eip712.rs`
- `crates/adapters/polymarket/src/execution/order_builder.rs`
- `crates/adapters/polymarket/src/execution/submitter.rs`
- `crates/adapters/polymarket/src/execution/parse.rs`
- `crates/adapters/polymarket/src/execution/mod.rs`
- `crates/adapters/polymarket/src/data.rs`
- `crates/adapters/polymarket/src/providers.rs`
- `crates/adapters/polymarket/src/filters.rs`
- `crates/adapters/polymarket/src/http/models.rs`
- `crates/adapters/polymarket/src/http/parse.rs`
- `crates/adapters/polymarket/src/http/query.rs`
- `crates/adapters/polymarket/src/common/consts.rs`
- `crates/adapters/polymarket/src/common/urls.rs`
- `crates/adapters/polymarket/src/common/models.rs`
- `crates/adapters/polymarket/src/common/parse.rs`
- `crates/adapters/polymarket/src/websocket/dispatch.rs`
- `crates/adapters/polymarket/src/websocket/parse.rs`
- `crates/adapters/polymarket/src/config.rs`
- `crates/adapters/polymarket/src/factories.rs`
- `crates/model/src/currencies.rs`
- `crates/model/src/reports/fill.rs`

Polymarket-relevant changes:

| Area | Upstream change | Bolt implication |
| --- | --- | --- |
| CLOB V2 signing | `DOMAIN_VERSION` changes to `"2"`; CTF exchange addresses change to V2 addresses; EIP-712 `Order` drops `taker`, `expiration`, `nonce`, and `feeRateBps`; adds `timestamp`, `metadata`, and `builder`. | This is the direct CLOB V2 blocker reducer. Bolt does not build Polymarket signed orders itself, so the signing fix is inside NT once pinned. |
| Wire order body | `PolymarketOrder` no longer includes `taker`, `nonce`, or `feeRateBps`; it includes `expiration`, `timestamp`, `metadata`, and `builder`. `expiration` is wire-only, not signed. | Bolt tests/docs that reason about V1 shape or fee-rate-in-order must be updated. |
| Collateral currency | NT adds `Currency::pUSD()` and Polymarket constants move from `USDC` to `PUSD`. Balances and fills now use pUSD. | Bolt docs and risk fields currently describe gross USDC entry-cost terms. Runtime semantics must decide whether Bolt-facing names stay "USDC terms" or become pUSD collateral terms. |
| Fee behavior | Pre-submit `OrderSubmitter::get_fee_rate_bps` cache is removed from the submit path. Commission uses instrument `feeSchedule.rate`; maker fills are zero-fee; market BUY can be adjusted against pUSD balance and fees through `MarketBuyFeeContext`. | Current runtime contract requires selected-side fee rate availability before entry. That contract becomes stale or at least incomplete under V2. Bolt must decide whether the V2 contract relies on NT's pUSD balance/fee-schedule context or on a stricter local preflight. |
| CLOB URLs | Default HTTP CLOB URL changes from `https://clob.polymarket.com` to `https://clob-v2.polymarket.com`; upstream comment says `clob.polymarket.com` also serves V2 after cutover and should be flipped later. | Bolt currently requires explicit URLs and docs/fixtures use `https://clob.polymarket.com`. Because Bolt overrides defaults, it will not inherit NT's default CLOB V2 preview host unless config/docs change. |
| Config fields | `PolymarketDataClientConfig` adds `auto_load_missing_instruments`, `auto_load_debounce_ms`, `transport_backend`; `PolymarketExecClientConfig` adds `transport_backend`. | Direct Bolt struct literals break until these fields are supplied explicitly. Do not rely on NT defaults for `auto_load_missing_instruments`; the current controlled-loading contract requires an explicit `false` unless a later design changes that contract. |
| Credentials | `crates/adapters/polymarket/src/common/credential.rs` is unchanged in the direct candidate commit; env fallbacks and info log marker remain. | Bolt's forbidden env-var checks and log suppression remain required. No V2-specific credential simplification found. |
| Factories | Polymarket factories now import `ClientConfig`, `DataClientFactory`, and `ExecutionClientFactory` from `nautilus_common::factories`, not `nautilus_system::factories`. | Bolt imports of `nautilus_system::factories` become compile errors across current and legacy/shared runtime support code. |
| Instrument loading | Data config default `auto_load_missing_instruments = true`; missing-instrument subscribe/request can trigger ad-hoc Gamma loads. Instruments carry pUSD currency, `fee_schedule`, `game_id`; `min_quantity` is unset because Polymarket has separate limit and market minima. | Bolt's controlled loading contract must explicitly disable auto-load in the probe. A later provider-boundary refactor must preserve this as a runtime-contract invariant rather than inheriting upstream default `true`. |
| Execution reports/fills | Fill commission uses instrument taker fee and liquidity side; pUSD commission currency; `OrderStatusReport.expire_time` is populated from V2 `expiration`; `FillReport` has `avg_px`. | Downstream state/risk code should verify pUSD commission and expiration behavior, even though Bolt does not currently submit orders in v3. |

Symbol breadcrumbs for review: `eip712.rs::DOMAIN_VERSION`, `eip712.rs::Order`, `http/models.rs::PolymarketOrder`, `consts.rs::PUSD`, `urls.rs::CLOB_HTTP_URL`, `currencies.rs::Currency::pUSD`, `execution/parse.rs::compute_commission`, `execution/submitter.rs::MarketBuyFeeContext`, `http/parse.rs::fee_schedule`, `http/parse.rs::game_id`, and `reports/fill.rs::FillReport::avg_px`.

## 4. Bolt Compile/API Impact Matrix

| Bolt file | Likely impact if pinned to candidate | Notes |
| --- | --- | --- |
| `Cargo.toml` / `Cargo.lock` / `rust-toolchain.toml` | Must change for an actual pin bump. Lockfile churn will be large. | Upstream workspace moves `0.55.0` to `0.56.0` and bumps Rust to `1.95.0`; Bolt currently has `rust-version = "1.94.1"` and `rust-toolchain.toml` channel `1.94.1`. Probe CI/local toolchains must satisfy `1.95.0`. Lockfile churn magnitude was not measured in this audit; quantify it in the probe before mixing in compile fix-up. |
| `src/bolt_v3_adapters.rs` | Compile break. | Constructs `PolymarketDataClientConfig`, `PolymarketExecClientConfig`, and `BinanceDataClientConfig` directly. Must add Polymarket data `auto_load_missing_instruments`, `auto_load_debounce_ms`, `transport_backend`; Polymarket exec `transport_backend`; Binance data `transport_backend`. If Binance execution config literals are introduced in this path, they must also supply `transport_backend`. |
| `src/bolt_v3_secrets.rs` | No compile break expected from Polymarket V2 itself. | Verified `crates/adapters/polymarket/src/common/credential.rs` has no diff between current pin and candidate. Forbidden env-var behavior remains necessary because NT credential env fallbacks still exist. |
| `src/bolt_v3_client_registration.rs` | Polymarket factory names still exist, but test direct data config literal breaks. | Main path passes cloned configs into `LiveNodeBuilder::add_*`; that API remains. Unit fixture must add new config fields. |
| `src/bolt_v3_live_node.rs` | Likely compiles for logging because it uses `..Default::default()`. Behavior should be rechecked. | `LiveNodeBuilder::build` now calls runtime-support validation; current defaults likely pass. Credential module path still exists. |
| `src/bolt_v3_providers/polymarket.rs` | Behavior/doc drift. | Schema lacks fields for `auto_load_missing_instruments`, debounce, and transport backend. Current explicit CLOB URLs override NT defaults. |
| `tests/bolt_v3_adapter_mapping.rs` | Compile and assertion updates. | Asserts old config field set and URLs; imports `SignatureType` still likely OK. |
| `tests/bolt_v3_client_registration.rs` | Compile break. | Direct `PolymarketDataClientConfig` literal missing new fields. |
| `tests/bolt_v3_provider_binding.rs` | Compile may pass after adapter fix, but behavior assertions should recheck filters under auto-load. | The test pins `filters` behavior; upstream auto-load could bypass intended loading policy if enabled. |
| `tests/bolt_v3_credential_log_suppression.rs` | Likely still relevant and should remain. | NT still logs `Polymarket credentials resolved...` from `nautilus_polymarket::common::credential`. |
| `docs/bolt-v3/2026-04-28-source-grounded-status-map.md` | Must update after probe evidence. | Row 46 currently says CLOB V2 commit was not verified in repo. |
| `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` | Must update if pin accepted. | Section 7.3's fee-rate availability requirement and Section 13's gate need V2 evidence. USDC terminology must be revisited. |
| `docs/bolt-v3/2026-04-25-bolt-v3-contract-ledger.md` | Must update after evidence. | Section 14 remains blocker until candidate pin is compiled and verified inside Bolt. |

Additional repo-wide likely compile breaks from the candidate:

- `src/clients/mod.rs`, `src/live_node_setup.rs`, `src/clients/chainlink.rs`, `tests/support/mod.rs`, `tests/reference_pipeline.rs`, `tests/platform_runtime.rs`, and `src/platform/runtime.rs` import `nautilus_system::factories`; candidate only exposes these traits from `nautilus_common::factories`.
- `src/clients/binance.rs` constructs `BinanceDataClientConfig` directly and will need the new `transport_backend` field. `src/bolt_v3_adapters.rs` also maps Binance data directly.
- Any full `LoggerConfig` literals would need new fields, but current Bolt occurrences use `..Default::default()`.

## 5. Bolt Behavior/Risk Impact Matrix

| Risk | Impact | Required decision |
| --- | --- | --- |
| CLOB V2 blocker | Reduced, not closed. | Close only after Bolt pins, compiles, and records V2 signing/domain/currency/fee evidence against the exact pinned commit. |
| pUSD vs USDC.e | Material semantic change. | Decide whether Bolt external contract renames notional fields to pUSD or documents pUSD as the collateral replacing prior gross USDC terms. |
| Fee readiness | Current contract is stale. | Replace "fee-rate fetch before entry" with a V2 policy based on Gamma `feeSchedule.rate`, maker/taker behavior, and market BUY fee adjustment. Explicitly decide whether Bolt relies on NT's optional `MarketBuyFeeContext` path or requires a local pUSD balance/fee preflight before order submission. |
| Explicit CLOB URLs | Bolt overrides NT defaults. | Decide whether config should switch to `https://clob-v2.polymarket.com` for the probe or keep `https://clob.polymarket.com` with cutover evidence. |
| Auto-load missing instruments | Could weaken controlled loading if defaulted on. | Apply the same rejection pattern as `subscribe_new_markets` in `src/bolt_v3_adapters.rs`: do not forward ad-hoc loading in the current controlled-loading contract, and set `auto_load_missing_instruments = false` explicitly in the probe. |
| Credential fallbacks/logging | Unchanged upstream. | Keep forbidden env-var checks and credential log suppression. |
| Factory trait move | Broad compile risk. | Probe should update imports mechanically before deeper architecture work. |
| Lockfile/dependency churn | Large. | Probe should isolate pin bump so dependency fallout is reviewable before production PR. |

## 6. Tests/Verifiers Likely Affected

Minimum tests to rerun in a probe after compile fixes:

- `cargo test --test bolt_v3_adapter_mapping`
- `cargo test --test bolt_v3_client_registration`
- `cargo test --test bolt_v3_provider_binding`
- `cargo test --test bolt_v3_credential_log_suppression`
- `cargo test --test live_node_run`
- `cargo test --test bolt_v3_controlled_connect`

Also recheck any signing/replay verifier or fixture that assumes V1 signed-order shape: V2 keeps `expiration` in the wire body but excludes it from the EIP-712 signed hash.

Likely additional affected tests because of the `nautilus_system::factories` move:

- `cargo test --test reference_pipeline`
- `cargo test --test platform_runtime`
- tests depending on `tests/support/mod.rs`

Docs/verifiers that must be updated only after probe evidence:

- status map row 46
- runtime contracts Section 7.3 and Section 13
- contract ledger Section 14

## 7. Recommended Next Slice

Do not start a compatibility probe from this audit step. The next sequence should be:

1. Verify this audit doc.
2. Request adversarial review of the audit and recommendation.
3. If the audit passes review, create a separate compatibility probe branch/worktree from `feat/236-bolt-v3-slice-8-1-baseline`.

The probe must not be based on `main`; `main` is missing the slice-8-1 / market-identity / source-grounded-status-map work represented by this branch. Local evidence at audit time: `git log --oneline main..feat/236-bolt-v3-slice-8-1-baseline` listed seven commits, including `0342346 docs(bolt-v3): add source-grounded status map`, while `git log --oneline feat/236-bolt-v3-slice-8-1-baseline..main` was empty.

Do not reuse the existing `probe/nt-polymarket-v2-compat` branch/worktree if it is still present. At audit time, `.worktrees/probe-nt-polymarket-v2-compat` was checked out at `d6bf66f62bc4835f12e903b1b4a793bbc53a58ce` and was missing the baseline commits above.

Probe scope:

- Bump NT rev to `56a438216442f079edf322a39cdc0d9e655ba6d8`.
- Update only mechanical compile fallout:
  - `nautilus_system::factories` imports to `nautilus_common::factories`
  - new Polymarket config fields
  - new Binance `transport_backend` fields where existing direct literals break
  - explicit `auto_load_missing_instruments = false`
  - explicit `transport_backend` values needed to compile; production readiness still requires TOML/schema/mapping evidence if this remains runtime-configurable
  - Rust `1.95.0` toolchain/Cargo metadata alignment required by the candidate
- Run the focused tests above.
- Record actual compiler/test evidence in a follow-up report.
- Do not include provider-boundary refactor.
- Do not include feature work.
- Do not include live trading behavior.

Do not combine the first probe with a provider adapter boundary refactor. That refactor is still desirable, but doing it before the compatibility probe would obscure which failures are pure NT API drift versus local architecture work.

After the probe, the production slice can be either:

- a narrow pin bump with audited compatibility fixes, if fallout is small; or
- provider adapter boundary refactor plus pin bump, if the probe proves the boundary is the cheapest way to carry the new NT fields and policy.

## 8. Explicit Non-Goals

- No Cargo pin bump in this audit.
- No `Cargo.lock` update.
- No merge.
- No push.
- No live trading or canary decision.
- No claim that Bolt-v3 is ready for live capital.
- No old Bolt v1 repo inspection.
- No secrets or `.env` inspection.
