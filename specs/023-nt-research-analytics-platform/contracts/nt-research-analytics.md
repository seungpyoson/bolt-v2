# Contract: NT-First Research Analytics Platform

## Authority

- `evidence.md` is the authority for claims.
- `spec.md`, `plan.md`, `research.md`, `data-model.md`, `tasks.md`, and issue payloads must cite or inherit ledger rows.
- A `GAP` cannot be described as implemented scope.
- A `USER_ASSUMPTION` can drive planning, but issue acceptance must say what proof will confirm or falsify it.

## Source-Of-Truth Chain

```text
Raw evidence record
  -> deterministic NT catalog projection
  -> NT BacktestNode / NT reports / NT events / NT snapshots
  -> analytics read model
  -> read-only dashboard / notebooks
```

Rules:

- Raw evidence is audit input, not trading truth.
- NT catalog is canonical replay/backtest input.
- NT reports/events/snapshots are trading truth for orders, fills, positions, PnL, account state, and portfolio state.
- Analytics read models are derived and must carry source hashes/freshness.
- Dashboards are read-only and must expose staleness.
- Dashboard outlook/strategy-state fields must be backed by an accepted source
  contract or omitted/labeled as non-trading-truth.
- Dashboard UI/tooling must pass a product-fit gate before bespoke UI work:
  Grafana for ops observability, Metabase/Preset/Superset for SQL BI, and
  Plotly/Dash for custom visual apps. Product choice cannot change source truth.

## Venue Gates

Venue/product/provider identity is selected through TOML-backed registry or
binding entries. The core runtime, admission path, secret path, research runner,
and dashboard must not branch on hardcoded venue names.

| Venue/surface | Planning stance | Required proof before implementation claim |
|---|---|---|
| Hyperliquid HIP-4 | Upstream NT-supported on `develop`; current Bolt pointer must be updated or selected. | Compile/API proof plus lifecycle matrix for instruments, orders, fills, settlement, reconciliation, and historical-data class. |
| Kalshi | Adapter support assumed by user instruction. | Exact source/pointer proof for adapter, data, execution, account/fill/report surfaces; historical L2 proof or lower-fidelity label. |
| Perpetual futures venues | Use NT live adapters only for source-proven venue/product surfaces; Tardis or official archives are candidates for historical replay. Checked venue examples are evidence instances, not special architecture paths. | Cost/fidelity gate and replay-to-catalog sample before selecting a provider; official venue capture needs official-source proof before use. |
| Polymarket | Use NT adapter/loader first. | Public API cap/depth proof before adding Telonex/Goldsky/other supplement. |
| Dashboard/PnL | NT reports/events/snapshots first. | #409 or equivalent `PortfolioSnapshot` capture proof, #77 durable trade-history/PnL path, and #36 inclusion/exclusion decision for redemption realized PnL before dashboard PnL completeness claims. |
| Dashboard/BI product | Existing products before custom UI. | Source-contract, security, query-backend, UX, and all-in monthly cost proof before selecting Grafana, Metabase, Preset/Superset, Plotly/Dash, or bespoke UI. |

## Provider Gates

- Tardis Professional replay access is `$900/month`; selection requires all-in monthly cost under cap or explicit waiver.
- Telonex Plus is personal-use priced; team/commercial use requires license/price proof.
- Goldsky is usage-metered; selection requires event/storage/query estimate.
- Official archives/APIs must be labeled by freshness and completeness.
- Official API/archive capture is a `GAP` per venue until source-proven.
- Forward capture cannot backfill historical L2 claims.
- Final provider selection must refresh price, license, and usage-limit evidence
  at selection time; planning-snapshot prices are not final acceptance evidence.

## Binding Contract Tests

- Venue/provider swaps must be represented by TOML and registry/binding data
  changes only.
- Contract tests must fail if core runtime, admission, secret resolution,
  research runner, catalog projection, analytics read model, or dashboard code
  branches on concrete venue or provider names.
- The same test fixture must exercise at least two venue/provider bindings so a
  single hardcoded happy path cannot satisfy the gate.

## Prohibited Claims

- Do not claim Kalshi adapter support is source-proven from the checked NT clone.
- Do not claim HIP-4 historical execution-quality backtesting from live adapter evidence alone.
- Do not claim Tardis is selected before all-in cost is calculated.
- Do not claim dashboard PnL completeness while `PortfolioSnapshot` remains uncaptured.
- Do not create a Bolt backtest engine, executable order schema, or venue translation layer without explicit evidence that NT cannot provide the needed surface.
- Do not hardcode concrete venue, product, provider, market, account, or credential identity into core runtime, research, or dashboard logic.

## Existing Issue Map

| Issue | Relation |
|---|---|
| #24 | Existing NT-first data lake follow-on epic; do not duplicate its ETL/lake scope. |
| #23 | Existing instrument spool bridge; dependency for complete reduced ETL seam. |
| #34 | Existing Polymarket strategy platform epic; dashboard/research should not silently expand it. |
| #36 | Existing redemption-realized-PnL/history issue; dashboard must link or explicitly exclude redemption scope. |
| #39 | Existing adaptive venue weighting issue; research analytics may feed it later, but should not make it baseline scope. |
| #112 | Existing Kalshi venue epic; new Kalshi proof slice should update or depend on it. |
| #77 | Existing durable trade-history/PnL path issue; dashboard historical PnL depends on it or must label the gap. |
| #115 | Existing HIP-4 issue with premise stale relative to upstream NT `develop`; update/link and prove selected Bolt pointer rather than duplicate or imply closure. |
| #148 | Existing inline-capture sidecar risk issue; comprehensive capture must respect its deferred trigger stance. |
| #236 | Thin NT rebuild epic; architecture parent for NT-first/no-dual-path constraints. |
| #409 | Existing `PortfolioSnapshot` capture issue; dashboard PnL completeness depends on it. |

## Review Contract

- Review the ledger first, then the prose artifacts.
- Findings should cite exact ledger row, source line, or missing proof.
- External review should challenge evidence classification, not seek consensus over prose.
- No issue creation or mutation without explicit user approval.
