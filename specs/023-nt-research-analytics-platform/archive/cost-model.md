# Cost Model: NT-First Research Planning Package

> Archive note: this file is historical/audit context. Live authority is
> `../reference/evidence.md`, `../reference/data-model.md`, and
> `../reference/contracts.md`.

**Status**: Draft planning artifact.
**Refreshed**: 2026-05-21.
**Cost posture**: choose the best-fidelity architecture first, then expose
all-in recurring cost and cut levers for user review.

## Decision Rule

Do not select a provider from price alone. A source is eligible only if:

- data fidelity satisfies the claim in `fidelity-matrix.md`;
- license allows intended use;
- recurring provider, AWS, dashboard, logs, query, transfer, and support costs
  are explicitly modeled;
- output can map to NT catalog/report vocabulary without a second trading truth;
- venue/product/provider identity remains TOML/registry-selected.

The original `$1000/month` target remains a review reference, but it is not a
reason to choose lower-fidelity data in this research package. Over-target
scenarios are marked for user review before implementation.

## Current Source Facts

| Source | Current cost/license fact | Evidence | Planning implication |
|---|---|---|---|
| Tardis Perpetuals Professional | `$900/month`; replay APIs and Tardis Machine start at Professional. | Tardis pricing shows perpetuals Professional at `$900/month` and replay/API rows in the same matrix: https://tardis.dev/ lines 271-363. Tardis billing docs say Pro/Business include raw replay API, Tardis Machine, and instrument metadata API: https://docs.tardis.dev/faq/billing-and-subscriptions lines 184-188, 229-247. | Technically strong crypto/perps candidate; likely over-target once AWS/dashboard are included, so model cost instead of rejecting it prematurely. |
| Tardis All Exchanges Professional | `$2200/month`. | Tardis pricing shows all 50+ spot/derivatives Professional at `$2200/month`: https://tardis.dev/ lines 692-784. | Broadest replay candidate in this set; requires explicit user review before implementation because provider price alone exceeds the original target. |
| Telonex Plus | `$79/month`, personal-use license for individual trading/research. | Telonex pricing: https://telonex.io/pricing lines 21-31. | Plausible Polymarket research source for personal use; team/commercial use needs Enterprise quote. |
| Telonex Enterprise | Custom price, commercial-use license. | Telonex pricing: https://telonex.io/pricing lines 33-43. | Required before team/production/commercial reliance. |
| Goldsky Starter/Scale | Starter free; Scale usage-based; pricing is metered by active subgraphs, stored entities, pipeline workers, event writes, hosted DB compute/storage, and RPC. | Goldsky pricing: https://docs.goldsky.com/pricing/summary lines 75-92, 95-159, 206-227. | Good on-chain provenance candidate; exact run-rate needs event/storage estimate before selection. |
| Hyperliquid official archive | Provider fee not sourced here; requester pays transfer costs; monthly uploads can be missing. | Hyperliquid docs: https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data lines 60-69. | Low provider-fee candidate but lower completeness/timeliness confidence; AWS transfer must be modeled. |
| Kalshi official historical API | No paid provider price sourced in this pass; historical endpoints exist for cutoff, markets, candlesticks, trades, fills, orders. | Kalshi docs: https://docs.kalshi.com/getting_started/historical_data lines 67-92. | Use for lower-fidelity historical classes unless adapter/source proves historical L2. |
| Polymarket official APIs | No paid provider price sourced in this pass; APIs cover discovery, CLOB orderbooks/prices/history, and data positions/trades/activity. | Polymarket docs: https://docs.polymarket.com/market-data/overview lines 176-212. | Useful baseline/source-of-truth API family, but public API cap/depth limits still gate fidelity. |
| Canonical S3 artifact root | Canonical raw payloads, NT catalog data, source proofs, and backtest outputs share one S3 root with typed subpaths. | E-034 user decision. AWS S3 pricing page points to pricing examples and calculator: https://aws.amazon.com/s3/pricing/ lines 576-578, 650-652. | Model storage, request, transfer, lifecycle, retention, and query costs together; do not hide them under provider cost. |
| S3 archive lifecycle | Retain forever, no automatic delete; colder S3 classes are lifecycle destinations for inactive artifacts. | E-035 user decision. AWS S3 docs state Deep Archive is the lowest-cost AWS storage option, has 180-day minimum duration, requires restore before access, and adds archive metadata overhead. | Treat archive storage under `$5/month` as zero for planning. Still model retrieval latency/cost, request/metadata overhead, and minimum-duration effects before relying on it operationally. |
| AWS dashboard/compute/query | Must use AWS Pricing Calculator for final estimate. | AWS S3 pricing: https://aws.amazon.com/s3/pricing/ lines 576-578, 650-652. | Treat AWS as explicit reserve, not hidden residual. |
| Grafana Cloud | Free visualization tier exists; Pro visualization is `$8/active user` plus `$19/month` platform fee, with usage-based observability costs. | Grafana pricing: https://grafana.com/pricing/ lines 1097-1159. | Strong ops metrics/logs candidate; do not use as independent trading truth. Model usage, retention, and alerting separately from data-provider cost. |
| Metabase | Open Source is `$0`; managed Starter is `$100/month + $6/user/month`; Pro is `$575/month + $12/user/month`; Enterprise starts at `$20k/year`. | Metabase pricing: https://www.metabase.com/pricing-plans/ lines 78-139. | Good SQL BI candidate over analytics/read-model DB; managed tiers can consume budget quickly and must be modeled separately from provider costs. |
| Preset Cloud / Superset | Preset Starter is `$0` up to 5 users; Professional is `$20/user/month` annually or `$25/user/month` monthly; embedded viewer licenses start at `$500/month`. Apache Superset is open-source BI. | Preset pricing: https://preset.io/pricing/ lines 24-61, 89-141. Superset site: https://superset.apache.org/ lines 44-60, 116-125. | Good managed/open-source BI candidate. Self-hosted Superset shifts cost to AWS/ops; Preset embedding can exceed reserve. |
| Retool | Free/Team/Business/Enterprise tiers for internal app/workflow builders and users; Business includes stronger controls such as audit logging and permissions. | Retool pricing: https://retool.com/pricing. | Good internal-tool candidate for operator workflows; model builder/user seats, workflow runs, audit/permission tier needs, and data-source/security fit before selection. |
| Plotly Cloud / Dash | Free tier exists; Pro is `$29/creator/month`; Enterprise is custom. | Plotly pricing: https://plotly.com/pricing/ lines 34-60, 390-399. | Good custom visual app candidate only if BI tools cannot satisfy workflow; otherwise risks unnecessary bespoke UI. |

## Candidate Monthly Envelopes

| Scenario | Included sources | Known monthly cost | Required reserve | Cap status | Blocking unknowns |
|---|---:|---:|---:|---|---|
| Crypto/perps L2 replay first | Tardis Perpetuals Professional + canonical S3 artifact root + AWS/dashboard reserve | `$900` provider | UNESTIMATED | OVER_TARGET_REVIEW | S3 storage/transfer/query/dashboard costs pending; selected venue replay proof pending. |
| Broad crypto all-exchange replay | Tardis All Exchanges Professional + canonical S3 artifact root + AWS/dashboard reserve | `$2200` provider | UNESTIMATED | OVER_TARGET_REVIEW | Requires explicit user review before implementation. |
| Polymarket personal research | Telonex Plus + official APIs + canonical S3 artifact root + AWS reserve | `$79` provider | UNESTIMATED | DECISION_NEEDED | Personal license only; S3/dashboard/log/query costs and NT projection/fidelity proof pending. |
| Polymarket commercial/team | Telonex Enterprise and/or Goldsky + canonical S3 artifact root + AWS reserve | Custom/metered | UNESTIMATED | DECISION_NEEDED | Enterprise quote or Goldsky event/storage estimate. |
| HIP-4/Hyperliquid official archive | Official archive/API + canonical S3 artifact root + AWS transfer/storage | No provider fee sourced | UNESTIMATED | DECISION_NEEDED | Missing/timeliness caveat; transfer/storage/modeling pending. |
| Kalshi baseline | Official historical API + adapter/provider proof + canonical S3 artifact root + AWS reserve | No provider fee sourced | UNESTIMATED | DECISION_NEEDED | Historical L2 not proven; adapter/source proof pending. |
| Dashboard/BI managed baseline | Grafana/Metabase/Preset/Retool/Plotly managed product + AWS/query reserve | `$0` to custom before usage | UNESTIMATED | DECISION_NEEDED | Must be selected from source contract, query backend, security, and cost fit; custom UI is fallback only. |

## Provisional Cost Guardrails

- Do not downshift the data source to satisfy a preliminary budget before the
  best-fidelity option is documented.
- If a scenario exceeds the original `$1000/month` target, mark it
  `OVER_TARGET_REVIEW` and record the expected all-in run rate, scope, cut
  levers, and revisit date before implementation.
- Treat Telonex Plus as personal research only until license proof says otherwise.
- Treat Goldsky as usage-metered: no selection before event count, entity count,
  pipeline worker, sink storage, and query estimate exist.
- Treat official archives/APIs as low provider-cost but fidelity-risky until
  source completeness and transfer costs are proven.
- Treat archive storage as retention-first, not free. No delete-by-default rule;
  use configured lifecycle profiles to move inactive artifacts colder. Archive
  storage under `$5/month` is planning-zero.
- Treat dashboard/BI as a product-selection gate. Use Grafana for ops
  observability, Metabase/Preset/Superset for SQL analytics, Retool for
  internal operator workflows, or Plotly/Dash for custom visual app only when
  source contract and cost review prove fit.
- If Tardis Perpetuals Professional is selected, managed BI tiers above free or
  tiny per-user plans should be modeled as separate cut levers, not hidden
  residual spend.

## Next Proof

1. Estimate selected-venue data volume per day and per month.
2. Estimate canonical S3 `artifact_root` storage, request, transfer, lifecycle,
   retention, Athena/DuckDB/ClickHouse/query, dashboard, and log costs.
3. Define lifecycle transition windows for `active`, `archive`, and
   `deep_archive`; verify no default expiration/delete rule exists.
4. Mark each scenario as best-fidelity, acceptable tradeoff, or cost-cut lever.
5. Request quotes or license confirmation for Telonex Enterprise if commercial
   use is intended.
6. Build Goldsky event/storage estimate before using it as Polymarket provenance
   backbone.
7. Select dashboard/BI product path from source contract, security, query
   backend, operational burden, and all-in monthly cost before custom UI work.
8. Refresh price, license, and usage-limit evidence at final provider
   pre-selection; dated planning snapshots are not final acceptance evidence.
