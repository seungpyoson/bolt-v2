# Cost Model: NT-First Research Analytics Platform

**Status**: Draft planning artifact.
**Refreshed**: 2026-05-20.
**Budget cap**: <= `$1000/month` for recurring provider + AWS + dashboard costs,
unless user explicitly waives the cap.

## Decision Rule

Do not select a provider from price alone. A source is eligible only if:

- data fidelity satisfies the claim in `fidelity-matrix.md`;
- license allows intended use;
- total recurring cost stays <= `$1000/month`, or cap waiver is explicit;
- output can map to NT catalog/report vocabulary without a second trading truth;
- venue/product/provider identity remains TOML/registry-selected.

## Current Source Facts

| Source | Current cost/license fact | Evidence | Planning implication |
|---|---|---|---|
| Tardis Perpetuals Professional | `$900/month`; replay APIs and Tardis Machine start at Professional. | Tardis pricing shows perpetuals Professional at `$900/month` and replay/API rows in the same matrix: https://tardis.dev/ lines 271-363. Tardis billing docs say Pro/Business include raw replay API, Tardis Machine, and instrument metadata API: https://docs.tardis.dev/faq/billing-and-subscriptions lines 184-188, 229-247. | Technically strong crypto/perps candidate, but leaves <= `$100/month` for AWS/dashboard unless cap waived. |
| Tardis All Exchanges Professional | `$2200/month`. | Tardis pricing shows all 50+ spot/derivatives Professional at `$2200/month`: https://tardis.dev/ lines 692-784. | Not eligible under cap without waiver. |
| Telonex Plus | `$79/month`, personal-use license for individual trading/research. | Telonex pricing: https://telonex.io/pricing lines 21-31. | Plausible Polymarket research source for personal use; team/commercial use needs Enterprise quote. |
| Telonex Enterprise | Custom price, commercial-use license. | Telonex pricing: https://telonex.io/pricing lines 33-43. | Required before team/production/commercial reliance. |
| Goldsky Starter/Scale | Starter free; Scale usage-based; pricing is metered by active subgraphs, stored entities, pipeline workers, event writes, hosted DB compute/storage, and RPC. | Goldsky pricing: https://docs.goldsky.com/pricing/summary lines 75-92, 95-159, 206-227. | Good on-chain provenance candidate; exact run-rate needs event/storage estimate before selection. |
| Hyperliquid official archive | Provider fee not sourced here; requester pays transfer costs; monthly uploads can be missing. | Hyperliquid docs: https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data lines 60-69. | Low provider-fee candidate but lower completeness/timeliness confidence; AWS transfer must be modeled. |
| Kalshi official historical API | No paid provider price sourced in this pass; historical endpoints exist for cutoff, markets, candlesticks, trades, fills, orders. | Kalshi docs: https://docs.kalshi.com/getting_started/historical_data lines 67-92. | Use for lower-fidelity historical classes unless adapter/source proves historical L2. |
| Polymarket official APIs | No paid provider price sourced in this pass; APIs cover discovery, CLOB orderbooks/prices/history, and data positions/trades/activity. | Polymarket docs: https://docs.polymarket.com/market-data/overview lines 176-212. | Useful baseline/source-of-truth API family, but public API cap/depth limits still gate fidelity. |
| AWS S3/dashboard/compute | Must use AWS Pricing Calculator for final estimate. S3 pricing page points to pricing examples and calculator. | AWS S3 pricing: https://aws.amazon.com/s3/pricing/ lines 576-578, 650-652. | Treat AWS as explicit reserve, not hidden residual. |
| Grafana Cloud | Free visualization tier exists; Pro visualization is `$8/active user` plus `$19/month` platform fee, with usage-based observability costs. | Grafana pricing: https://grafana.com/pricing/ lines 1097-1159. | Strong ops metrics/logs candidate; do not use as independent trading truth. Tardis `$900` mode leaves too little room for careless usage. |
| Metabase | Open Source is `$0`; managed Starter is `$100/month + $6/user/month`; Pro is `$575/month + $12/user/month`; Enterprise starts at `$20k/year`. | Metabase pricing: https://www.metabase.com/pricing-plans/ lines 78-139. | Good SQL BI candidate over analytics/read-model DB; managed Starter already consumes the full Tardis residual reserve before AWS. |
| Preset Cloud / Superset | Preset Starter is `$0` up to 5 users; Professional is `$20/user/month` annually or `$25/user/month` monthly; embedded viewer licenses start at `$500/month`. Apache Superset is open-source BI. | Preset pricing: https://preset.io/pricing/ lines 24-61, 89-141. Superset site: https://superset.apache.org/ lines 44-60, 116-125. | Good managed/open-source BI candidate. Self-hosted Superset shifts cost to AWS/ops; Preset embedding can exceed reserve. |
| Plotly Cloud / Dash | Free tier exists; Pro is `$29/creator/month`; Enterprise is custom. | Plotly pricing: https://plotly.com/pricing/ lines 34-60, 390-399. | Good custom visual app candidate only if BI tools cannot satisfy workflow; otherwise risks unnecessary bespoke UI. |

## Candidate Monthly Envelopes

| Scenario | Included sources | Known monthly cost | Required reserve | Cap status | Blocking unknowns |
|---|---:|---:|---:|---|---|
| Crypto/perps L2 replay first | Tardis Perpetuals Professional + AWS/dashboard reserve | `$900` provider | <= `$100` | DECISION_NEEDED | AWS transfer/storage/query/dashboard must fit reserve; selected venue replay proof pending. |
| Broad crypto all-exchange replay | Tardis All Exchanges Professional + AWS/dashboard reserve | `$2200` provider | UNESTIMATED | BLOCKED_UNDER_CAP | Needs explicit waiver. |
| Polymarket personal research | Telonex Plus + official APIs + AWS reserve | `$79` provider | UNESTIMATED | DECISION_NEEDED | Personal license only; AWS/dashboard/log/query costs and NT projection/fidelity proof pending. |
| Polymarket commercial/team | Telonex Enterprise and/or Goldsky + AWS reserve | Custom/metered | UNESTIMATED | DECISION_NEEDED | Enterprise quote or Goldsky event/storage estimate. |
| HIP-4/Hyperliquid official archive | Official archive/API + AWS transfer/storage | No provider fee sourced | UNESTIMATED | DECISION_NEEDED | Missing/timeliness caveat; transfer/storage/modeling pending. |
| Kalshi baseline | Official historical API + adapter/provider proof + AWS reserve | No provider fee sourced | UNESTIMATED | DECISION_NEEDED | Historical L2 not proven; adapter/source proof pending. |
| Dashboard/BI managed baseline | Grafana/Metabase/Preset/Plotly managed product + AWS/query reserve | `$0` to custom before usage | UNESTIMATED | DECISION_NEEDED | Must be selected from source contract, query backend, security, and cap fit; custom UI is fallback only. |

## Provisional Budget Guardrails

- If Tardis Perpetuals Professional is selected, hard-cap AWS/dashboard/log/query
  reserve at `$100/month` unless user waives cap.
- If Tardis All Exchanges is needed, request waiver before implementation.
- Waiver process: record explicit user approval in the provider-selection issue
  or review artifact with approved monthly cap, expected all-in run rate, scope,
  and expiration/revisit date.
- Treat Telonex Plus as personal research only until license proof says otherwise.
- Treat Goldsky as usage-metered: no selection before event count, entity count,
  pipeline worker, sink storage, and query estimate exist.
- Treat official archives/APIs as low provider-cost but fidelity-risky until
  source completeness and transfer costs are proven.
- Treat dashboard/BI as a product-selection gate. Use Grafana for ops
  observability, Metabase/Preset/Superset for SQL analytics, or Plotly/Dash for
  custom visual app only when source contract and cap prove fit.
- If Tardis Perpetuals Professional is selected, managed BI tiers above free or
  tiny per-user plans likely need a waiver or a self-hosted/open-source path.

## Next Proof

1. Estimate selected-venue data volume per day and per month.
2. Estimate S3 storage, transfer, Athena/DuckDB/ClickHouse/query, dashboard, and
   log costs.
3. Decide whether Tardis Perpetuals fits within `$900 + <=$100` reserve.
4. Request quotes or license confirmation for Telonex Enterprise if commercial
   use is intended.
5. Build Goldsky event/storage estimate before using it as Polymarket provenance
   backbone.
6. Select dashboard/BI product path from source contract, security, query
   backend, operational burden, and all-in monthly cost before custom UI work.
