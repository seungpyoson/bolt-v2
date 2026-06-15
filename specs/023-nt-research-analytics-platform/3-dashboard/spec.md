# Spec: Dashboard

## Scope

Build a future read-only operator dashboard over NT-derived sources. It shows
current trades, positions, PnL, exposure, data health, strategy state, and
outlook only where source contracts exist.

Primary customer job is trade investigation: view ongoing trades, prior
trades/fills, why trades fired, which strategy/signal/source binding produced
them, respective PnL, historical context, and data/proof freshness.

This is separate from Backtesting Engine and Research Analytics. It does not own
trading actions, credential mutation, provider capture, backtest execution, or
independent PnL/account truth.

## Users

- Operator: inspects current state, freshness, exposure, PnL, and data health.
- Maintainer: verifies each displayed field has an accepted source and no
  mutation path.

## Requirements

- Dashboard must be read-only: no order submit, cancel, transfer, credential, or
  runtime mutation action.
- Future dashboard work must classify customer jobs and write capabilities
  before product selection. Non-trading annotation/review workflow writes may be
  considered only after explicit artifact kind/schema/owner/audit rules exist.
  Trading, runtime config, credential, and funds/order mutations remain outside
  this package unless a separate future scope explicitly approves them.
- Every displayed field must map to NT report, NT event, NT snapshot, NT catalog
  Arrow output (read via duckdb/polars), or explicit unavailable/gap label.
- PnL, positions, orders, fills, exposure, and account state must come from
  NT-derived sources. #409 `PortfolioSnapshot` is the single PnL read source;
  dashboard must not compute independent account or MTM truth.
- Freshness/staleness must be visible for live/current fields.
- Dashboard PnL completeness requires #409 or equivalent `PortfolioSnapshot`
  capture proof, #77 durable PnL/history path, and #36 redemption-PnL inclusion
  or exclusion decision.
- If a required PnL or exposure source is missing, Dashboard must either omit
  that field or render it with an explicit partial/unavailable gap label. It
  must not invent independent account, PnL, exposure, or MTM truth.
- Existing BI/observability/internal-tool products must be evaluated before
  bespoke UI: Grafana, Metabase, Superset/Preset, Retool, Plotly/Dash, and
  custom fallback.
- Product choice must not change source truth.
- Outlook/strategy-state fields require accepted source contracts or must be
  omitted/labeled non-trading-truth; dashboard must not calculate them as
  trading truth.
- Strategy state/outlook must not be presented as trading truth unless an
  accepted source contract exists. Pre-proof research outlook may be displayed
  only as exploratory/non-trading-truth.
- Venue/provider identity must remain TOML/registry-selected data; dashboard
  field-source resolution must not branch on hardcoded venue/provider names.

## Evidence And Decisions

| Row | Status | Meaning for this project |
|---|---|---|
| E-017 | SOURCE_PROVEN | NT reports, events, and `PortfolioSnapshot` are the correct dashboard truth sources. |
| E-018 | SOURCE_PROVEN | Bolt currently lacks `PortfolioSnapshot` capture; account-wide MTM/PnL completeness is blocked. |
| E-019 | SOURCE_PROVEN | Runtime capture expansion has operational risk; do not casually add live capture in dashboard work. |
| E-020 | SOURCE_PROVEN | #409, #77, #36, and #369 are existing dependencies for dashboard/PnL claims. |
| E-023 | DECISION_NEEDED | Outlook and strategy-state sources are not yet source-proven as trading truth. |
| E-024 | USER_ASSUMPTION + DECISION_NEEDED | Choose best source/product fit first; model all-in dashboard/product cost for user review instead of defaulting to weaker architecture. |
| E-026 | SOURCE_PROVEN | Venue/product/provider identity is configuration and registry data, not dashboard logic. |
| E-028 | SOURCE_PROVEN + DECISION_NEEDED | Grafana, Metabase, Superset/Preset, Retool, Plotly/Dash, and custom UI are candidates; product choice requires proof. |
| E-029 | SOURCE_PROVEN | Live credentials must remain AWS SSM-only; dashboard must never display, mutate, or bypass the live secret path. |
| E-031 | SOURCE_PROVEN | Mature systems separate research/backtest/live and analysis views; dashboard is not a trading control plane. |

## Product Gate

- Grafana: first candidate for ops metrics/logs and time-series observability.
- Metabase or Superset/Preset: candidates for SQL analytics/read-model
  dashboards.
- Retool: candidate for internal operator workflows only if permissions,
  auditability, source-contract, query/API, security, UX, and cost fit.
- Plotly/Dash: candidate only when custom visual app behavior is needed.
- Bespoke UI: fallback after rejecting product candidates with source-contract,
  security, query-backend, UX, cost, and ops-burden evidence.

## Data Model

- `DashboardFieldSource`: field name, source type, source ref (NT report/event/
  snapshot, catalog Arrow URI via duckdb/polars, or `PortfolioSnapshot` id),
  fidelity class, freshness rule, source role, data status/gap reason, and
  user-facing legend key.
- `TradeExplanationField`: trade/order/fill id, source binding, strategy id,
  signal/reason evidence refs, and PnL source stance and freshness/gap labels
  for drilldown.
- `FreshnessRule`: max age, display behavior, alert behavior, and source ref.
- `DashboardReadModel`: read-only projection derived from NT/report/analytics
  sources.
- `NoMutationControl`: forbidden action, enforcement mechanism, and test.
- `ProductGateDecision`: candidate product, source-contract fit, query backend,
  security, UX, cost, ops burden, and decision.

## Issue Dependencies

Link or depend on #36, #77, #88, #148, #236, #369, and #409. Dashboard source
contract work does not close production readiness by itself.

GitHub issue #733 records the dashboard source-contract scope and links the
named dependency set for implementation review.

## Non-Goals

- No trading or runtime control plane.
- No independent PnL/account calculations.
- No backtest runner.
- No research notebook/experiment workflow.
- No provider capture or data lake writer.

## Acceptance

- Reviewer can trace every dashboard field to an accepted source or gap label.
- Reviewer can see stale/delayed/unavailable behavior for each live field.
- Reviewer can prove the selected product/UI has no mutation capability.
- Reviewer can see BI/product evaluation before custom UI work.
