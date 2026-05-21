# Plan: Dashboard

## Architecture

```text
NT reports / NT events / NT snapshots / accepted analytics tables
  -> DashboardReadModel
  -> read-only BI product or custom UI
  -> field-level freshness and gap labels
```

Dashboard truth is derived. NT reports/events/snapshots remain authority for
trading state. Analytics tables may support views but cannot replace NT-derived
truth for PnL, positions, fills, orders, account state, or portfolio state.

## Source Roles And Data Status

Keep source role separate from data status. Exact user-facing label names and
legend text require a shared terminology pass before UI implementation.

Source roles:

- `authoritative`: NT report/event/snapshot source.
- `derived`: read model computed from authoritative or accepted analytics source.
- `exploratory`: research/outlook field, not trading truth.

Data status/gap reasons:

- `current`: source is within freshness threshold.
- `stale`: source exists but freshness threshold is exceeded.
- `partial`: source exists but coverage is incomplete.
- `unavailable`: required source is missing or blocked.
- `excluded`: intentionally out of scope.
- `non_normal_run`: reproduction, audit, regression, or migration result; not
  normal current performance.

Additional source rules:

- Artifact links from Backtesting Engine or Research Analytics must point under
  the shared S3 `artifact_root`; dashboard products do not own canonical
  artifact storage.
- Dashboard may display explicit artifact-local handles passed by upstream
  producers; cross-run and bulk artifact lists use committed Artifact Index
  snapshots. Dashboard products do not recursively scan S3 prefixes as their
  normal discovery path.
- Dashboard is read-only for Artifact Index records. It may render upstream
  artifact links and lifecycle state, but it must not publish, repair, invent,
  or mutate artifact index truth.
- Dashboard may render source proof ids, fidelity classes, claim limits, and
  warnings from upstream artifacts/results. It must not accept upstream proof,
  weaken forbidden claims, mutate accepted proof records, or reclassify/upgrade
  proof strength. It preserves proof version/supersession metadata and does not
  relabel historical results after proof supersession. It may display
  `run_purpose`, `proof_pin_reason_code`, and `proof_pin_reason_detail` for
  non-latest proof pins.
- Dashboard may render strategy-review or promotion status only when it comes
  from `PromotionPackage` or later RA-owned review artifacts. It must not infer
  that status from backtest metrics or mutate it.
- Missing PnL or exposure sources must produce omitted fields or explicit
  partial/unavailable labels. Dashboard must not compute independent account,
  PnL, exposure, or MTM truth.
- Strategy state/outlook must be omitted unless an accepted source contract
  exists, or rendered only as exploratory/non-trading-truth. Dashboard must not
  calculate strategy state or outlook as trading truth.
- Lifecycle/restore status may be displayed for artifact links, but dashboard
  products must not delete, expire, or mutate canonical artifacts.

## Implementation Gates

1. Define dashboard customer jobs and capability classes.
2. Build field-source matrix.
3. Resolve #409, #77, #36, and #369 dashboard/PnL dependencies.
4. Define freshness/staleness behavior for each live/current field.
5. Run product gate for Grafana, Metabase, Superset/Preset, Retool,
   Plotly/Dash, and custom fallback.
6. Define no-mutation controls for selected product/UI.
7. Validate read-only source contract before UI implementation.
8. Validate displayed artifact links resolve under shared S3 `artifact_root` and
   that cross-run/bulk lists use committed Artifact Index snapshots when
   artifact lists are in scope.
9. Validate dashboard has no artifact delete/expiration controls.

## Customer Jobs And Capability Classes

Product choice is deferred until these jobs are specified and weighted:

1. Trade monitor: ongoing trades/orders, positions, exposure, current PnL,
   venue/source binding, and freshness.
2. Trade investigation: prior trades/fills, why trade fired,
   strategy/signal/reason refs, source proof/data used, and historical PnL
   context.
3. Annotation/review notes: optional notes, tags, comments, and investigation
   status. This is least necessary and requires explicit owner/schema/audit
   rules before any write path.
4. Controlled action workflow: request rerun, request RA review, stage config
   review, or future high-risk actions. Trading/runtime/credential/fund/order
   mutation remains outside this package unless separately approved.

## Field Source Matrix Seed

| Field group | Required source stance |
|---|---|
| Orders/fills/positions | NT reports/events/snapshots only. |
| Trade explanation | Strategy/signal/reason evidence refs, source proof refs, and source binding from accepted upstream artifacts; never inferred by dashboard. |
| Account state and portfolio equity | `PortfolioSnapshot` or explicit unavailable label until #409/equivalent lands. |
| Exposure | NT reports/events/snapshots or accepted derived analytics table with freshness; otherwise omit or render explicit partial/unavailable label. |
| Historical PnL | Durable trade-history/PnL path from #77 or omit/render explicit gap label. |
| Redemption-realized PnL | Include only after #36 scope decision, otherwise mark excluded/unavailable. |
| Strategy state/outlook | Accepted source contract or omit/render exploratory/non-trading-truth label. |
| Strategy review/promotion status | RA-owned `PromotionPackage` or review artifact only; never inferred from BTE metrics. |
| Data health/freshness | Source timestamp plus configured stale threshold. |

## Product Gate

Evaluate products against the customer jobs and read-model shape before
selection:

1. Grafana for ops metrics/logs and time-series observability.
2. Metabase or Superset/Preset for SQL analytics/read-model dashboards and
   trade investigation tables/drilldowns.
3. Retool for internal operator workflows, annotations, or controlled action
   requests if permissions, auditability,
   source-contract, query/API, security, UX, and cost fit.
4. Plotly/Dash for custom visual app needs.
5. Bespoke UI only after source-contract, security, query-backend, UX, cost, and
   operational burden justify it.

## Product Cost Baselines

These are planning snapshots and must be refreshed before product selection.

| Product path | Known cost | Dashboard implication |
|---|---:|---|
| Grafana Cloud | Free tier exists; Pro visualization is `$8/active user` plus `$19/month` platform fee, with usage-based observability costs | Strong ops metrics/logs candidate; not trading truth. |
| Metabase | Open Source `$0`; managed Starter `$100/month + $6/user/month`; Pro `$575/month + $12/user/month`; Enterprise starts at `$20k/year` | Good SQL BI candidate; managed tiers can consume budget quickly. |
| Preset/Superset | Preset Starter `$0` up to 5 users; Professional `$20/user/month` annually or `$25/user/month` monthly; embedded viewers start at `$500/month`; Superset is open source | Good managed/open-source BI candidate; self-hosting shifts cost to AWS/ops. |
| Retool | Free, Team, Business, and Enterprise tiers; Business includes stronger controls such as audit logging and permissions | Candidate for internal operator workflows; model builder/user seats, workflow runs, permissions/audit tier, and source-security fit. |
| Plotly/Dash | Free tier exists; Pro `$29/creator/month`; Enterprise custom | Use only if BI products cannot satisfy visual workflow. |
| Custom UI | No fixed product fee; engineering and ops cost unestimated | Fallback only after product gate rejects existing tools. |

## Issue Payload

Title: `Plan: read-only current trades, PnL, outlook dashboard source contract`

Accepted scope: define field-by-field source matrix, dependency decisions for
#409/#77/#36/#369, product gate, freshness behavior, and no-mutation controls.

Out of scope: building dashboard UI, building dashboard read model, independent
PnL calculation, and production readiness closure.

## Test Plan

- Field matrix tests fail for unmapped displayed fields.
- Freshness tests fail when stale data is rendered as current.
- Permission/route tests fail on any mutation action.
- Product gate cannot choose custom UI without rejecting product candidates with
  evidence.
- Artifact-link tests fail if dashboard creates or assumes a second canonical
  artifact root or uses recursive S3 listing as the normal discovery path.
- Lifecycle tests fail if dashboard adds artifact delete/expiration controls.

## Residual Risks

- Account-wide MTM/PnL remains incomplete until `PortfolioSnapshot` capture is
  accepted.
- Historical PnL remains incomplete until durable trade-history/PnL path is
  accepted.
- Outlook/strategy-state may remain exploratory unless a source contract is
  accepted.
- Product pricing and security posture can drift and must be refreshed at
  selection time.
