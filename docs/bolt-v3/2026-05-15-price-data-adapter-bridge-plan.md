# Bolt-v3 Price/Data Adapter Bridge Plan

Date: 2026-05-15

## Preservation Note

Preserved from an untracked root-worktree artifact into PR #388 on 2026-05-18.
The repo anchor below is historical; revalidate against current `main` before
using this as an implementation plan.

## Repo Anchor

- Source of truth: current `main`.
- `git status --short --branch`: `## main...origin/main`.
- `git rev-parse HEAD origin/main`: both `cece0f22c6b0e2a0c9141fd7325f720bff452911`.
- Scope: inventory and planning only. No Rust production code changed.
- Old bolt v1 repo: not read.

## Evidence Summary

- Legacy reference venue enum includes `Binance`, `Bybit`, `Deribit`, `Hyperliquid`, `Kraken`, `Okx`, `Polymarket`, `Chainlink`; see `src/config.rs:31-40`.
- Legacy reference builder routes Binance, Bybit, Chainlink, Deribit, Hyperliquid, Kraken, OKX; Polymarket falls into unsupported `other`; see `src/platform/runtime.rs:364-385`.
- Legacy reference client names are fixed uppercase strings for Binance, Bybit, Deribit, Hyperliquid, Kraken, OKX, Chainlink; Polymarket reuses the configured primary Polymarket data client name; see `src/platform/runtime.rs:684-700`.
- Reference fusion classifies Chainlink as `Oracle`; all other reference venues default to `Orderbook`; see `src/platform/reference.rs:220-226`.
- Bolt-v3 provider bindings currently register only `polymarket` and `binance`; see `src/bolt_v3_providers/mod.rs:115-138`.
- Bolt-v3 client registration uses TOML `[venues.<id>]` keys as NT registration names, not fixed uppercase per-kind names; see `src/bolt_v3_client_registration.rs:83-125`.

## Adapter Inventory

| Adapter | Existing builder/source file | Current surface | Data support | Execution support | Secrets needed | Hardcode / Phase 9 dependency | Recommended bolt-v3 bridge action |
|---|---|---|---|---|---|---|---|
| Binance | Legacy `src/clients/binance.rs:7-24`; v3 `src/bolt_v3_providers/binance.rs:258-300` | Both: legacy reference data and bolt-v3 provider data | Yes. Legacy resolves shared reference config; v3 maps `[venues.<id>.data]` to `BinanceDataClientConfig` | No in current bolt-v3. `validate_venue` rejects `[execution]`; see `src/bolt_v3_providers/binance.rs:142-148` | API key and API secret from SSM in both surfaces; see `src/secrets.rs:420-449` and `src/bolt_v3_providers/binance.rs:208-256` | Legacy client ID `"BINANCE"` is hardcoded in `reference_client_name_for_kind`. V3 provider key `"binance"` is binding-owned; fixture id `binance_reference` is TOML | Keep current v3 data binding. If refactoring later, factor shared Binance data mapping so legacy and v3 cannot diverge. Do not add Binance execution in data bridge PR |
| Bybit | `src/clients/bybit.rs:5-10` | Legacy reference path only | Yes, data-only default `BybitDataClientConfig` | None in this repo | None in current builder | Legacy client ID `"BYBIT"` hardcoded; no v3 provider key/binding | After Phase 9, add v3 provider/reference binding around existing builder. Do not rebuild adapter from scratch |
| Deribit | `src/clients/deribit.rs:5-10` | Legacy reference path only | Yes, data-only default `DeribitDataClientConfig` | None in this repo | None in current builder | Legacy client ID `"DERIBIT"` hardcoded; no v3 provider key/binding | After Phase 9, add v3 provider/reference binding around existing builder. Do not rebuild adapter from scratch |
| Hyperliquid | `src/clients/hyperliquid.rs:7-12` | Legacy reference path only | Yes, data-only default `HyperliquidDataClientConfig` | None in this repo | None in current builder | Legacy client ID `"HYPERLIQUID"` hardcoded; no v3 provider key/binding | After Phase 9, add v3 provider/reference binding around existing builder. Do not rebuild adapter from scratch |
| Kraken | `src/clients/kraken.rs:5-10` | Legacy reference path only | Yes, data-only default `KrakenDataClientConfig` | None in this repo | None in current builder | Legacy client ID `"KRAKEN"` hardcoded; no v3 provider key/binding | After Phase 9, add v3 provider/reference binding around existing builder. Do not rebuild adapter from scratch |
| OKX | `src/clients/okx.rs:5-10` | Legacy reference path only | Yes, data-only default `OKXDataClientConfig` | None in this repo | None in current builder | Legacy client ID `"OKX"` hardcoded; no v3 provider key/binding | After Phase 9, add v3 provider/reference binding around existing builder. Do not rebuild adapter from scratch |
| Chainlink | `src/clients/chainlink.rs:193-265` | Legacy reference path only | Yes. Custom `ChainlinkReferenceDataClient` emits `ChainlinkOracleUpdate`; see `src/clients/chainlink.rs:107-160` | None | API key and API secret from SSM; see `src/secrets.rs:408-417` | Client name `"CHAINLINK"`, metadata keys, WS path, and v3 feed version constant live in `src/clients/chainlink.rs:50-57` | After Phase 9, add a v3 oracle/data-feed binding around the existing Chainlink builder or an extracted shared builder. Preserve one shared-client/multi-feed shape. Do not rebuild from scratch |
| Polymarket | Legacy data `src/clients/polymarket.rs:381-456`; legacy exec `src/clients/polymarket.rs:1153-1179`; v3 provider `src/bolt_v3_providers/polymarket.rs:412-443` | Both: legacy data/exec and bolt-v3 provider data/exec | Yes. Legacy and v3 both map to `PolymarketDataClientFactory` | Yes. Legacy and v3 both map to `PolymarketExecutionClientFactory` | Execution needs private key, API key, API secret, passphrase from SSM; data-only has no secrets in v3. See `src/bolt_v3_providers/polymarket.rs:61-70` and `src/bolt_v3_providers/polymarket.rs:351-410` | Legacy default client name `"POLYMARKET"` in `src/live_config.rs:48-50`; v3 fixture id `polymarket_main` is TOML. Provider key `"polymarket"` is binding-owned | No new bridge needed for baseline. Keep execution changes out of data-adapter work. Any execution expansion needs separate venue validation |

## Current bolt-v3 Registration Facts

- `polymarket` v3 provider supports `[data]` and `[execution]`; `REQUIRED_SECRET_BLOCKS` applies to execution; see `src/bolt_v3_providers/polymarket.rs:59-78`.
- `binance` v3 provider supports `[data]` only; `execution: None` is returned by mapping; see `src/bolt_v3_providers/binance.rs:52-65` and `src/bolt_v3_providers/binance.rs:258-275`.
- Fixture proves current registered v3 surface: `polymarket_main` has data + execution, `binance_reference` has data-only; see `tests/fixtures/bolt_v3/root.toml:108-153`.
- Integration test proves the v3 LiveNode build path registers Polymarket data, Polymarket execution, and Binance data; see `tests/bolt_v3_client_registration.rs:26-85`.
- Do not claim Bybit, Deribit, Hyperliquid, Kraken, OKX, or Chainlink are production-enabled in bolt-v3 today. They are not in `PROVIDER_BINDINGS`.

## Bridge Rule

Bolt-v3 should reuse existing adapter builders wherever possible. It should not rebuild Bybit, Deribit, Hyperliquid, Kraken, OKX, or Chainlink adapters from scratch.

The bridge is provider/reference binding work, not adapter implementation work. After Phase 9 hardcode remediation lands, add bolt-v3 provider or reference bindings around existing builders, then register them through the existing `map_bolt_v3_adapters` -> `register_bolt_v3_clients` path.

Data-only wiring lands before any new execution wiring. Execution adapters require separate venue-specific validation, secret contract review, no-submit readiness, and live-canary gating. Execution must not be smuggled into a data-adapter PR.

## Sequencing

Before Phase 9:

- Keep this as doc/inventory only.
- Do not add new provider keys, client IDs, or fallback names.
- Do not port legacy uppercase reference client IDs into bolt-v3.
- Do not duplicate Chainlink or exchange data adapter code.

Immediately after Phase 9:

- Add data-only v3 binding tests first for one legacy reference adapter at a time.
- Start with data-only exchange reference bindings whose existing builders require no secrets: Bybit, Deribit, Hyperliquid, Kraken, OKX.
- Add Chainlink separately as oracle/data-feed binding because its current builder is a shared multi-feed custom `DataClient`.
- Keep Binance as existing v3 data provider and reconcile any duplicate mapping with the legacy Binance data builder only if tests prove divergence risk.
- Keep Polymarket as existing v3 data + execution provider; do not change its execution surface in the data bridge.

Later execution / trade expansion:

- Add execution only in separate venue-specific slices.
- For each venue, prove NT adapter capability, SSM secret fields, forbidden env-var blocklist, credential log filters, no-submit readiness, controlled connect/disconnect, and live-canary behavior.
- If NT lacks the needed execution adapter behavior, record a blocker or fix NT; do not implement a Bolt-owned venue adapter.

## Do Not Do Yet

- Do not wire Bybit, Deribit, Hyperliquid, Kraken, OKX, or Chainlink into bolt-v3 production before Phase 9 lands.
- Do not create a parallel adapter registry or second config format.
- Do not copy-paste legacy default-config builders into new v3 code.
- Do not hardcode new provider keys or client IDs outside provider binding modules.
- Do not introduce Binance execution.
- Do not introduce Chainlink execution; Chainlink is oracle/data-feed style.
- Do not treat legacy reference builders as proof of bolt-v3 production enablement until they are registered through `src/bolt_v3_providers/mod.rs`, `src/bolt_v3_adapters.rs`, and `src/bolt_v3_client_registration.rs`.

## Verification Performed

- `git status --short --branch` -> clean `main...origin/main`.
- `git rev-parse HEAD origin/main` -> both `cece0f22c6b0e2a0c9141fd7325f720bff452911`.
- Source inspection with `rg`, `probe symbols`, and line-scoped `nl -ba ... | sed -n`.
- Spec-kit prerequisite check: `.specify/scripts/bash/check-prerequisites.sh --json --require-tasks --include-tasks` returned `ERROR: Not on a feature branch. Current branch: main`; this is expected for this task because the user required current `main` as source of truth.
- Focused tests: `cargo test --test reference_pipeline --test bolt_v3_adapter_mapping --test bolt_v3_client_registration --test bolt_v3_provider_binding` -> 34 passed, 0 failed.
- Claude Code custom review job `69549164-4e2f-4ec8-8710-23db356aae33`: source sent, status completed, verdict approved, no blocking findings.
