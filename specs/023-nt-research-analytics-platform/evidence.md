# Evidence Ledger: NT-First Research Analytics Platform

This file is the control surface for the plan. `spec.md`, `plan.md`,
`research.md`, tasks, and issue payloads must not assert anything stronger than
the status recorded here.

## Source Anchors

- Bolt main checked in this session: `831368756bf5a7f8398944502dcce5fcc7c7952d`
  equals `origin/main`.
- Bolt currently pins NT crates to
  `7c2aafb30fb143069c915a3f2057bb12174405f6`
  ([Cargo.toml](../../Cargo.toml:22), [Cargo.toml](../../Cargo.toml:51)).
- Upstream NT `develop` was fetched on 2026-05-20 and resolved to
  `dbf4d8c90af06f0f1f1e56d8b5130ada763f1953`.
- User instruction: Kalshi adapter support is to be assumed. Treat that as a
  planning assumption, not source proof from the checked upstream clone.

## Source Labels

- `NT upstream`: local checkout `/private/tmp/nt-upstream-develop` at
  `dbf4d8c90af06f0f1f1e56d8b5130ada763f1953`; durable URL base
  `https://github.com/nautechsystems/nautilus_trader/blob/dbf4d8c90af06f0f1f1e56d8b5130ada763f1953/`.
- `Tardis pricing`: `https://tardis.dev/`, fetched 2026-05-20, lines 271-363,
  692-784.
- `Tardis billing`: `https://docs.tardis.dev/faq/billing-and-subscriptions`,
  fetched 2026-05-20, lines 229-247.
- `Hyperliquid historical data`:
  `https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data`, fetched
  2026-05-20, lines 67-69.
- `Kalshi Historical Data`:
  `https://docs.kalshi.com/getting_started/historical_data`, fetched
  2026-05-20, lines 76-92 and 97-110.
- `Kalshi Historical Trades`:
  `https://docs.kalshi.com/api-reference/historical/get-historical-trades`,
  fetched 2026-05-20, lines 157-188.
- `Kalshi Historical Market Candlesticks`:
  `https://docs.kalshi.com/api-reference/historical/get-historical-market-candlesticks`,
  fetched 2026-05-20, lines 196-198.
- `Kalshi Market Orderbook`:
  `https://docs.kalshi.com/api-reference/market/get-market-orderbook`, fetched
  2026-05-20, lines 191-193.
- `Polymarket Introduction`: `https://docs.polymarket.com/api-reference`,
  fetched 2026-05-20, lines 203-228.
- `Telonex docs`: `https://telonex.io/docs/`, fetched 2026-05-20, lines 39-51.
- `Telonex pricing`: `https://telonex.io/pricing`, fetched 2026-05-20, lines
  21-43.
- `Goldsky Pricing`: `https://docs.goldsky.com/pricing`, fetched 2026-05-20,
  lines 79-92, 97-120, and 123-159.
- `AWS S3 Pricing`: `https://aws.amazon.com/s3/pricing/`, fetched 2026-05-20,
  lines 576-578 and 650-652.
- `GitHub issues`: fetched/refreshed 2026-05-20 with `gh issue view`/`gh issue list` from
  `https://github.com/seungpyoson/bolt-v2/issues/{number}` for #19, #20, #21,
  #22, #23, #24, #34, #36, #39, #75, #77, #88, #112, #115, #127, #148, #158,
  #176, #236, #254, #369, #385, #407, and #409.
  `gh issue list`/`gh issue view` showed all of those issues open on
  2026-05-20.

## Status Vocabulary

- `SOURCE_PROVEN`: exact source/doc/issue evidence exists.
- `USER_ASSUMPTION`: user supplied the premise; build from it, but do not call
  it source-proven.
- `GAP`: evidence shows missing or incomplete coverage, or no source proof was
  found.
- `DECISION_NEEDED`: more than one valid path remains; user or follow-on gate
  must choose.
- Combined statuses are allowed when one row carries mixed facts, for example
  `USER_ASSUMPTION + GAP`.

## Claims

| ID | Claim | Status | Evidence | Implication | Next proof |
|---|---|---|---|---|---|
| E-001 | Do not build a Bolt-owned backtest engine before using NT. | SOURCE_PROVEN | NT docs define `BacktestEngine` and recommend `BacktestNode` for production workflows and live transition; high-level API requires a Parquet catalog: upstream `docs/getting_started/index.md:38-46` at `dbf4d8c...`. `BacktestNode` connects `ParquetDataCatalog` to `BacktestEngine`: upstream `crates/backtest/src/node.rs:42-43`. | Research runner should orchestrate NT runs, not simulate venue/order lifecycle. | Compile selected NT pointer; prove Bolt manifest maps to `BacktestRunConfig`/`BacktestDataConfig`. |
| E-002 | NT catalog is the replay/backtest projection target. | SOURCE_PROVEN | NT docs describe data catalog as Parquet central store for backtesting and live scenarios: upstream `docs/concepts/data.md:716-737`. `BacktestDataConfig` requires `catalog_path` and data class: upstream `docs/concepts/data.md:953-963`; Rust config carries catalog path, protocol/storage options, instrument filters, start/end, and filter expressions: upstream `crates/backtest/src/config.rs:595-635`. | Raw provider data can be retained, but canonical replay input should be deterministic NT catalog projection. | Define raw-evidence-to-NT-catalog lineage fields and hash checks. |
| E-003 | Upstream NT `develop` supports Hyperliquid HIP-4 outcome markets for data and trading. | SOURCE_PROVEN | Release notes add HIP-4 instruments, reconciliation, userOutcome actions, and Settlement fill parsing: upstream `RELEASES.md:113-115`. Hyperliquid docs list HIP-4 Outcomes as data feed and trading supported: upstream `docs/integrations/hyperliquid.md:117-125`. | #115's older premise that NT lacks HIP-4 is stale relative to upstream `develop`; do not build a Bolt HIP-4 adapter first. | Update/select NT pointer and run compile/API proof. |
| E-004 | HIP-4 ordinary outcome orders use NT's standard order path. | SOURCE_PROVEN | Docs state outcome side tokens trade through `SubmitOrder` and the same `Order` action, with no HIP-4-specific call for ordinary orders: upstream `docs/integrations/hyperliquid.md:377-383`. Smoke test states the same purpose: upstream `crates/adapters/hyperliquid/bin/http_outcome_order.rs:16-22`. | Strategy/order path should stay NT-native for ordinary buy/sell. | Prove one no-submit/testnet order lifecycle after pointer update. |
| E-005 | HIP-4 instruments are modeled as USDH `BinaryOption` side tokens. | SOURCE_PROVEN | Docs: two `BinaryOption` instruments per outcome, USDH settlement/currency registration: upstream `docs/integrations/hyperliquid.md:361-375`. `HyperliquidProductType` includes `Outcome`: upstream `crates/adapters/hyperliquid/src/common/enums.rs:979-1008`. | Bolt config should reference NT instrument IDs and product type, not custom HIP-4 instrument names. | Verify exact Rust API for loading outcome product type from TOML-driven config. |
| E-006 | HIP-4 settlement and userOutcome support exist upstream, but default settlement path nuance matters. | SOURCE_PROVEN | Docs say venue `Settlement` fills close side-token balances through standard user-fills stream: upstream `docs/integrations/hyperliquid.md:435-450`. Position reconciliation covers `+E`/`#E`: upstream `docs/integrations/hyperliquid.md:452-461`. UserOutcome action types exist: upstream `crates/adapters/hyperliquid/src/common/enums.rs:789-810`. Optional Rust-only settlement polling is disabled by default because venue fills drive settlement: upstream `crates/adapters/hyperliquid/src/config.rs:166-171` and `docs/integrations/hyperliquid.md:955-966`. | Do not write "settlement solved" as a blanket claim; prove chosen path and default behavior. | Lifecycle test matrix: instrument load, order submit, fill report, position report, settlement, userOutcome helper calls. |
| E-007 | HIP-4 live adapter support is separate from HIP-4 historical backtest-data support. | GAP | Hyperliquid official historical data warns monthly uploads may be missing and only L2 book snapshots plus asset contexts are in the archive; other datasets must be recorded through API: Hyperliquid docs `historical-data` lines 67-69. | HIP-4 can be live-supported while historical execution-quality backtests remain unproven. | Determine whether HIP-4 outcome books/fills appear in official archive, Tardis, or require forward capture. |
| E-008 | Kalshi adapter support is assumed by instruction, not proven by the checked upstream clone. | USER_ASSUMPTION + GAP | User instruction: "consider that Kalshi adapter is supported. Assume and build from there." Session command `rg -n "Kalshi|kalshi" /private/tmp/nt-upstream-develop/crates /private/tmp/nt-upstream-develop/docs /private/tmp/nt-upstream-develop/nautilus_trader` returned no hits. Existing #112 says no local Kalshi implementation, but that issue is not current NT-upstream proof. | Plan from supported Kalshi adapter; do not spend planning effort on inventing a Kalshi adapter unless pointer proof contradicts the assumption. | After pointer/update is known, prove adapter crate/module, data client, exec client, instruments, account/fill/report surfaces. |
| E-009 | Kalshi historical API covers markets/candles/trades/fills/orders, but historical L2 orderbook replay is not source-proven. | SOURCE_PROVEN + GAP | Kalshi docs list historical endpoints for cutoff, markets, market candlesticks, trades, fills, and orders: Kalshi Historical Data lines 76-92. Kalshi historical trades endpoint covers trades older than cutoff: Historical Trades lines 157-188. Historical candlesticks support 1m/1h/1d: Historical Market Candlesticks lines 196-198. Current orderbook endpoint is current book only: Market Orderbook lines 191-193. | Kalshi backtests must be labeled by fidelity. Candles/trades/fills are not the same as historical L2 execution replay. | Prove whether the assumed Kalshi adapter exposes archived orderbook deltas/snapshots; otherwise plan forward capture or lower-fidelity class. |
| E-010 | NT Tardis replay can produce catalog-compatible Parquet for supported crypto venues. | SOURCE_PROVEN | NT Tardis docs map normalized book/trade/quote/bar/instrument/funding formats to NT data types: upstream `docs/integrations/tardis.md:37-52`. Venue mapping includes BYBIT and HYPERLIQUID: upstream `docs/integrations/tardis.md:117-138`. Replay docs write one Parquet per instrument/data/date in catalog-compatible format: upstream `docs/integrations/tardis.md:169-185`; source writes deltas/depths/quotes/trades/bars: upstream `crates/adapters/tardis/src/replay.rs:153-160`, `228-260`, `593-626`. | Tardis is a strong NT-native candidate for venue-agnostic crypto historical replay where venue support, cost, and data classes pass proof gates. | Cost gate plus small replay-to-catalog proof per selected venue. |
| E-011 | Tardis Professional replay access leaves little monthly cap margin. | SOURCE_PROVEN + DECISION_NEEDED | Tardis pricing page shows Perpetuals Professional at `$900/month` with raw replay/Tardis Machine APIs in the visible pricing matrix: Tardis pricing lines 271-363. Tardis docs show raw data replay API and Tardis Machine APIs are only on Professional/Business: Billing docs lines 229-247. | Under a <= $1000/month all-in cap, Tardis Professional leaves little room for AWS, dashboard, logs, and query costs. All Exchanges Professional is over cap at `$2200/month` before AWS. | Prove exact AWS/dashboard reserve before selecting Tardis, or request waiver. |
| E-012 | Bybit live market data/execution support exists in NT, but venue-specific data-class claims require proof. | SOURCE_PROVEN | NT Bybit docs list data client, execution client, and live factories: upstream `docs/integrations/bybit.md:14-24`. This row proves the live adapter surface, not exact Bybit perp data classes. | Treat Bybit as one venue instance under the same venue-proof contract, not as an architecture anchor. | Verify selected venue product types and exact data classes before venue-specific implementation claims. |
| E-013 | Polymarket support exists in NT, but full historical coverage has a public API cap caveat. | SOURCE_PROVEN | Bolt pins `nautilus-polymarket`: [Cargo.toml](../../Cargo.toml:31). Upstream provider loads instruments via Gamma API: upstream `crates/adapters/polymarket/src/providers.rs:35-42`. Execution submitter accepts Nautilus-native types and posts to CLOB: upstream `crates/adapters/polymarket/src/execution/submitter.rs:101-109`. Loader docs source trades from Polymarket Data API and warn high-activity pagination cap can require another historical source: upstream `docs/integrations/polymarket.md:1101-1108`. | Use NT Polymarket first; use third-party historical data only for proven cap/depth gaps. | Decide Telonex/Goldsky/official forward capture after cost/license/fidelity gate. |
| E-014 | Polymarket official APIs split discovery, data, and CLOB orderbook/pricing/trading. | SOURCE_PROVEN | Polymarket docs list Gamma API for markets/events, Data API for positions/trades/activity/open interest, and CLOB API for orderbook/pricing/price history plus trading: Polymarket Introduction lines 203-228. | Raw evidence store may need multiple Polymarket source families; NT projection must preserve provenance. | Define source-family metadata in raw evidence and lineage. |
| E-015 | Telonex is a cheap Polymarket historical data candidate for individual use; team/commercial license is not the same price. | SOURCE_PROVEN + DECISION_NEEDED | Telonex docs say Polymarket is active with trades, quotes, book snapshots, onchain fills, crypto prices: Telonex docs lines 39-51. Pricing shows Plus at `$79/month` with personal use license, Enterprise custom with commercial use license: Telonex pricing lines 21-43. | Telonex may fit cap for personal research, but license gate is mandatory before team/production use. | Decide whether personal research is acceptable or request enterprise/commercial quote. |
| E-016 | Goldsky can support Polymarket on-chain/provenance indexing but is usage-metered. | SOURCE_PROVEN + DECISION_NEEDED | Goldsky docs: Starter free and Scale pay-as-you-go: Goldsky Pricing lines 79-92. Subgraph compute/storage and Mirror/Turbo billing are metered: lines 97-120 and 123-159. | Goldsky is not a blanket "free data lake"; use only when on-chain provenance is needed and run-rate is modeled. | Estimate events/subgraphs/pipelines/storage for selected Polymarket scope. |
| E-017 | Current live/dashboard truth can start from NT events, reports, and portfolio snapshots. | SOURCE_PROVEN | NT reports generate DataFrames from orders/fills/positions/account states for analysis and visualization: upstream `docs/concepts/reports.md:3-19`, `426-428`. `PortfolioSnapshot` carries mark-to-market totals, unrealized PnL, realized PnL, and total equity: upstream `crates/model/src/events/portfolio/snapshot.rs:31-65`. NT msgbus exposes subscribe/publish portfolio snapshot: upstream `crates/common/src/msgbus/api.rs:469-480`, `1087-1093`. | Dashboard should be read-only and derived from NT reports/events/snapshots, not recompute trading truth independently. | Implement/track capture gap for `PortfolioSnapshot` before claiming dashboard PnL completeness. |
| E-018 | Bolt currently knows `PortfolioSnapshot` capture is missing. | SOURCE_PROVEN | #409 says `subscribe_portfolio_snapshot` publishes on `events.portfolio.{account_id}`, Bolt does not currently subscribe, and acceptance requires runtime-capture subscription/persistence. Repo YAML also marks `PortfolioSnapshot` JSONL-feasible but not captured: [storage-feasibility.yaml](../../docs/bolt-v3/research/runtime-capture/storage-feasibility.yaml:93). | Dashboard/current-trade work depends on #409 or an equivalent slice. | Either implement #409 first or make dashboard explicitly omit account-wide MTM until captured. |
| E-019 | Runtime capture is already local-catalog constrained and covers selected stream classes, but expanding capture has operational risk. | SOURCE_PROVEN | `wire_nt_runtime_capture` ensures local catalog path and creates spool/jsonl paths: [src/nt_runtime_capture.rs](../../src/nt_runtime_capture.rs:662). Per-instrument stream classes include quotes, trades, order book deltas/depths, index/mark prices, instrument closes, instruments: [src/nt_runtime_capture.rs](../../src/nt_runtime_capture.rs:353). #148 records inline capture failure can stop the live node and sidecar extraction is deferred. | Comprehensive capture should not be casually expanded inside live trading without failure-mode review. | Decide per-stream whether inline capture is acceptable, sidecar is needed, or provider batch replay is safer. |
| E-020 | Existing issue coverage is real but not sufficient for a single broad "research platform" issue. | SOURCE_PROVEN | `issue-audit.md` records live issue checks. #24 covers NT-first data lake follow-ons and folds #19, #20, #21, #22, and #23. #34, #75, #88, and #39 cover related strategy/research consumers. #36, #77, #369, and #409 cover PnL/live observability dependencies. #112 covers Kalshi venue integration. #115 covers HIP-4. #127, #254, and #407 constrain Polymarket data/readiness. #148 and #158 constrain capture expansion. #236 is the thin-NT hard-reset epic. | New issues should update/link existing issues and fill missing slices only: cost/fidelity gate, NT support proof, research runner, analytics read model, dashboard source contract, and dashboard MVP. PnL/dashboard issue payloads must link #36, #77, #369, and #409 before claiming completeness. | Keep issue map refreshed before any issue mutation. |
| E-021 | Hyperliquid live data/trading support covers perps in upstream NT docs. | SOURCE_PROVEN | Hyperliquid docs list Perpetual Futures as data feed and trading supported, alongside HIP-3, spot, and HIP-4 outcomes: upstream `docs/integrations/hyperliquid.md:117-125`. | Hyperliquid perps should use NT adapter for live surfaces; historical replay still needs provider/fidelity gate. | Verify exact product-type config and current Bolt pointer support. |
| E-022 | Official API/archive capture must be source-proven per venue; Bybit is currently not source-proven in this ledger. | GAP | This pass collected NT Bybit adapter evidence and NT Tardis replay evidence, but no official Bybit API/archive source label. | Do not claim official archive/API suitability for any venue until an official-source check is added for that venue. Bybit remains one unresolved instance, not a special architecture path. | Fetch current official API/archive docs for each selected venue before selecting an official capture path. |
| E-023 | Dashboard "outlook" and strategy-state sources are not yet source-proven as trading truth. | DECISION_NEEDED | User requested current outlook in the dashboard. E-017 proves NT reports/events/snapshots for orders, fills, positions, account state, portfolio state, and PnL, but this ledger has no source proving a dedicated outlook feed or strategy-state event contract. | Dashboard source contract may include an outlook slot only as a read-only derived/decision-needed source until proven. UI MVP must omit it or label it as non-trading-truth if no accepted source exists. | Define source contract and source proof for strategy state/outlook before UI completeness claims. |
| E-024 | Recurring provider plus AWS plus dashboard cost must stay under the approved monthly cap unless explicitly waived. | USER_ASSUMPTION | User objective says cost is generally not an issue, but total spend should not exceed `$1000/month` including AWS unless waived. | Provider selection must model all-in recurring cost and request a waiver before selecting a mode above the cap. | Produce cost model with provider, AWS, dashboard, logs, query, transfer, and reserve components. |
| E-025 | Python/Jupyter is research-only and cannot become the production trading runtime. | SOURCE_PROVEN | Repo rules require a pure Rust binary with no Python layer: [AGENTS.md](../../AGENTS.md:25). Constitution Principle VII allows research notebooks only as research surfaces and requires productionized strategy behavior to graduate into the production runtime contract: [.specify/memory/constitution.md](../../.specify/memory/constitution.md:58). | Notebooks may analyze catalogs/results, but they cannot submit, mutate credentials, own strategy truth, or become the live node. | Define promotion gate from notebook finding to typed TOML/NT runtime config before any production use. |
| E-026 | Venue/product/provider identity is configuration and registry data, not core code. | SOURCE_PROVEN | Constitution Principle II requires venue-agnostic core and says provider keys, market-family keys, strategy archetypes, and NT adapter bindings live only in registry or binding modules selected by TOML: [.specify/memory/constitution.md](../../.specify/memory/constitution.md:26). Repo rules forbid hardcoded runtime values and require one coherent config section for changing a venue: [AGENTS.md](../../AGENTS.md:21), [AGENTS.md](../../AGENTS.md:27). | Research/backtest/dashboard slices must use generic venue gates and TOML-selected bindings. Concrete venue rows are evidence instances, not architecture branches. | Add schema/contract tests that switching a selected venue changes TOML/registry entries only, not core runtime, admission, secret, or dashboard logic. |
| E-027 | Current Bolt NT pin is verified, but manifest enablement is not the same as upstream capability. | SOURCE_PROVEN + DECISION_NEEDED | `Cargo.toml` pins NT crates to `7c2aafb30fb143069c915a3f2057bb12174405f6` and includes `nautilus-persistence`, `nautilus-polymarket`, `nautilus-binance`, and `nautilus-portfolio`; `Cargo.lock` resolves the same NT SHA. Local checkout `nautilus_trader-3c6af4345b4d438b/7c2aafb` resolves to that SHA. The checkout contains `nautilus-backtest`, `nautilus-hyperliquid`, `nautilus-tardis`, and perps adapters, but Bolt `Cargo.toml`/`Cargo.lock` do not currently reference all of those crates directly. | Upstream NT support should be used, but each planned surface needs selected pointer plus manifest/feature proof before implementation. Missing manifest dependency is a build/config gate, not permission to build a Bolt-owned duplicate engine or adapter. | Prove selected pointer and direct/indirect crate enablement for `nautilus-backtest`, `nautilus-hyperliquid`, `nautilus-tardis`, and any selected perps adapter before coding. |
| E-028 | Dashboard/analytics should evaluate existing products before custom UI. | SOURCE_PROVEN + DECISION_NEEDED | Grafana Cloud visualization has free and Pro active-user pricing: https://grafana.com/pricing/ lines 1097-1159. Metabase has Open Source, Starter, Pro, and Enterprise pricing: https://www.metabase.com/pricing-plans/ lines 78-139. Preset Cloud has free Starter, paid Professional, and embedded-dashboard add-on pricing: https://preset.io/pricing/ lines 24-61, 89-141. Apache Superset is an open-source data exploration/dashboard platform: https://superset.apache.org/ lines 44-60. Plotly Cloud/Dash pricing includes free, Pro, and custom Enterprise tiers: https://plotly.com/pricing/ lines 34-60, 390-399. | Dashboard MVP must run a product-fit gate before building bespoke UI. Source truth still comes from NT-derived read model; products are view/query surfaces only. | Select Grafana/Metabase/Preset/Superset/Plotly/custom from source contract, query backend, security, cost, and UX proof. |
| E-029 | Live credentials must remain AWS SSM-only. | SOURCE_PROVEN | Repo rules require SSM as the single secret source with Rust AWS SDK resolution: [AGENTS.md](../../AGENTS.md:26). Constitution Principle III says credentials resolve only from AWS SSM through the Rust AWS SDK: [.specify/memory/constitution.md](../../.specify/memory/constitution.md:32). | Research/backtest/dashboard config may reference credential keys, but live credential resolution must stay in the production Rust SSM path. | Schema and runtime tests must reject environment-variable fallbacks, API-key literals, or alternate secret backends before any live credential use. |

## Immediate Conclusions

1. HIP-4 should be planned as "NT upstream-supported; pointer/update proof needed."
2. Kalshi should be planned as "adapter supported by user assumption; fidelity
   proof still required."
3. Tardis is technically strong for source-proven crypto/perps replay, but
   cannot be selected before per-venue fidelity proof and all-in cost modeling.
4. Historical-data fidelity must be per venue and per source. Live adapter support
   does not prove historical L2 replay.
5. Dashboard scope depends on NT-derived capture first, especially #409, #77,
   and any #36 redemption-realized-PnL scope it chooses to show.
6. Concrete venues are selected through config/registry gates. Evidence examples
   must not become hardcoded branches in core runtime, research, or dashboard code.
7. Dashboard UI should use existing BI/observability products when they fit;
   custom UI requires explicit rejection of product candidates.

## Requirement Trace

| Requirement | Evidence rows | Status for planning |
|---|---|---|
| FR-001 NT backtest first | E-001 | SOURCE_PROVEN |
| FR-002 NT catalog/core types first | E-002 | SOURCE_PROVEN |
| FR-003 Thin run manifests | E-001, E-002 | SOURCE_PROVEN for NT target; DECISION_NEEDED for exact manifest schema |
| FR-004 Cost gate | E-011, E-015, E-016, E-024, E-028 | SOURCE_PROVEN for vendor price facts; USER_ASSUMPTION for cap; DECISION_NEEDED for selected mode |
| FR-005 Fidelity gate | E-007, E-009, E-010, E-013, E-021, E-022 | SOURCE_PROVEN need; GAP/DECISION_NEEDED per venue |
| FR-006 Kalshi adapter assumption and data-fidelity proof | E-008, E-009 | USER_ASSUMPTION + GAP for adapter; SOURCE_PROVEN/GAP for official historical data classes |
| FR-007 Use NT HIP-4 first | E-003, E-004, E-005, E-006 | SOURCE_PROVEN upstream; pointer proof pending |
| FR-008 Separate HIP-4 live and historical proof | E-003, E-007 | SOURCE_PROVEN split; historical is GAP |
| FR-009 Raw evidence -> NT catalog -> reports/results -> dashboard | E-002, E-014, E-017 | SOURCE_PROVEN architecture basis; lineage schema DECISION_NEEDED |
| FR-010 No independent PnL/account truth | E-017, E-018, E-020 | SOURCE_PROVEN need; capture and durable PnL/history gaps remain |
| FR-011 Read-only dashboard | E-017, E-018, E-020, E-023, E-028 | SOURCE_PROVEN source basis; product/UI/outlook design DECISION_NEEDED |
| FR-012 Dashboard freshness/staleness | E-017, E-018, E-019, E-020, E-023 | SOURCE_PROVEN need; exact freshness contract DECISION_NEEDED |
| FR-013 Python/Jupyter research-only | E-025 | SOURCE_PROVEN boundary; DECISION_NEEDED for enforcement mechanism |
| FR-014 Existing issue lookup | E-020 | SOURCE_PROVEN |
| FR-015 Third-party providers when better | E-010, E-011, E-015, E-016, E-022, E-028 | SOURCE_PROVEN options; selected provider/product DECISION_NEEDED; official capture remains venue-by-venue GAP until proven |
| FR-016 TOML/NT config, SSM live secrets, no hardcoded venue identity | E-026, E-027, E-029 | SOURCE_PROVEN repo rule and constitution boundary; manifest proof pending |
| FR-017 Classify every claim | This ledger | SOURCE_PROVEN process requirement |
