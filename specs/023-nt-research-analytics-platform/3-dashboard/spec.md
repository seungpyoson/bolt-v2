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
- Every displayed field must map to NT report, NT event, NT snapshot, derived
  analytics table, or explicit unavailable/gap label.
- PnL, positions, orders, fills, exposure, and account state must come from
  NT-derived sources.
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
- Backtest/source-proof artifact links displayed by dashboard must reference
  the configured S3 `artifact_root`; dashboard must not create a second canonical
  artifact root.
- Dashboard may display explicit artifact-local handles passed by upstream
  producers; cross-run and bulk artifact lists must come from committed
  Artifact Index snapshots, not recursive S3 listing.
- Dashboard is read-only for Artifact Index records and must not publish,
  repair, invent, or mutate upstream artifact truth.
- Dashboard may display `SourceProofReport` ids, fidelity classes, claim
  limits, and warnings, but must not reclassify source/backtest proof strength.
- Dashboard must not mark upstream `SourceProofReport` records accepted or
  weaken forbidden claims.
- Dashboard must preserve source proof version/supersession metadata and must
  not mutate accepted proof records.
- Dashboard must display historical results against the proof version they used;
  supersession may be surfaced as metadata but must not relabel old results as
  if they used the newer proof.
- Dashboard may display `proof_pin_reason_code` and `proof_pin_reason_detail`
  for non-latest proof pins when present.
- Dashboard may display `run_purpose` and must not present reproduction,
  audit, regression, or migration results as normal current results.
- Dashboard may display Research Analytics strategy-review or promotion status
  only from experiment-result verdict/promotion-config fields or later RA-owned
  review artifacts. It must not infer "use/escalate this strategy" from
  `BacktestResultContract` metrics or mutate promotion state.
- Dashboard must preserve artifact lifecycle status and must not add delete or
  expiration behavior for canonical artifacts.
- Dashboard user-facing labels, status names, and legends must be finalized in a
  cross-project terminology/legend pass before UI implementation. Internal semantics
  must distinguish source role from data status/gap reason.

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
| E-034 | USER_ASSUMPTION + DECISION_NEEDED | Dashboard may display artifact pointers from Backtesting Engine or Research Analytics, but canonical artifacts stay under the configured S3 `artifact_root`. |
| E-035 | USER_ASSUMPTION + DECISION_NEEDED | Dashboard may show lifecycle/restore status, but it must not delete, expire, or mutate canonical artifacts. |
| E-036 | USER_ASSUMPTION + DECISION_NEEDED | Dashboard may display lifecycle state, but lifecycle remains the cross-project simple rule: active, configured quiet window, inactive. |
| E-038 | SOURCE_PROVEN + DECISION_NEEDED | Dashboard may display explicit artifact-local handles, but cross-run/bulk artifact lists must consume committed Artifact Index snapshots; dashboard must not scan S3 prefixes as the normal discovery path. |
| E-039 | USER_ASSUMPTION + DECISION_NEEDED | Dashboard is a read-only Artifact Index consumer; it must not publish, repair, invent, or mutate upstream artifact records. |
| E-040 | USER_ASSUMPTION + DECISION_NEEDED | Dashboard may display source proof ids, fidelity classes, claim limits, and warnings, but it must not accept upstream proof, weaken forbidden claims, or upgrade proof strength. |
| E-041 | SOURCE_PROVEN + DECISION_NEEDED | Dashboard may display RA-owned strategy review status, but must not infer promotion from BTE result metrics or mutate promotion state. |

## Product Gate

<!-- dashboard-capability-boundary-ids: no_trading_runtime_credential_fund_order_mutation -->

- Grafana: first candidate for ops metrics/logs and time-series observability.
- Metabase or Superset/Preset: candidates for SQL analytics/read-model
  dashboards.
- Retool: candidate for internal operator workflows only if permissions,
  auditability, source-contract, query/API, security, UX, and cost fit.
- Plotly/Dash: candidate only when custom visual app behavior is needed.
- Bespoke UI: fallback after rejecting product candidates with source-contract,
  security, query-backend, UX, cost, and ops-burden evidence.

## Data Model

- `DashboardFieldSource`: field name, source type, source ref or cross-project
  `artifact_root` URI, source proof id if applicable, fidelity class, claim
  limits, run purpose, proof pin reason code/detail when present, lifecycle
  status, promotion/status source if applicable, freshness rule, source role,
  data status/gap reason, and user-facing legend key.
- `TradeExplanationField`: trade/order/fill id, source binding, strategy id,
  signal/reason evidence refs, source proof id/version, artifact refs, PnL
  source stance, and freshness/gap labels for drilldown.
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
