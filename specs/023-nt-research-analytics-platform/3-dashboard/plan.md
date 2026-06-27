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
legend text come from the canonical cross-project registry in
`../reference/contracts.md`.

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
  the configured S3 `artifact_root`; dashboard products do not own canonical
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
  from experiment-result verdict/promotion-config fields or later RA-owned
  review artifacts. It must not infer that status from backtest metrics or
  mutate it.
- Missing PnL or exposure sources must produce omitted fields or explicit
  partial/unavailable labels. Dashboard must not compute independent account,
  PnL, exposure, or MTM truth.
- Strategy state/outlook must be omitted unless an accepted source contract
  exists, or rendered only as exploratory/non-trading-truth. Dashboard must not
  calculate strategy state or outlook as trading truth.
- Lifecycle/restore status may be displayed for artifact links, but dashboard
  products must not delete, expire, or mutate canonical artifacts.

## Dependency Decisions

- #409 / `PortfolioSnapshot`: account state and portfolio equity are complete
  only when a `PortfolioSnapshot` source is present. Otherwise dashboard must
  omit the field or render `unavailable`/`partial` with an explicit gap reason.
- #77 durable trade-history/PnL: historical PnL is complete only when the
  durable trade-history/PnL source is present. Otherwise dashboard must render a
  gap label and must not reconstruct independent PnL truth.
- #36 redemption-realized PnL: excluded from the accepted dashboard read model
  until the owner explicitly includes it. The field must render `excluded` with
  a scope-excluded gap reason when present.
- #369 is non-closure context. A dashboard source contract, read model, or BI
  choice does not close production readiness, runtime capture, or trading
  control-plane readiness.

## Read-Only Source Contract Validation

The current implementation surface is a Rust read-model contract validator, not
a UI. It rejects proof-strength reclassification, accepted proof mutation,
forbidden-claim weakening, historical-result relabeling after proof
supersession, RA verdict derivation from BTE metrics, RA verdict/finding review
mutation, dashboard mutation actions, canonical artifact delete/expire/publish/
repair actions, cross-root artifact links, and cross-kind bulk lists built from
independent latest pointers rather than committed Artifact Index snapshots.

Field resolution uses `source_binding_key` values from config/registry data. It
rejects resolving fields through venue or provider identity.

## Implementation Gates

1. Define dashboard customer jobs and capability classes.
2. Build field-source matrix.
3. Resolve #409, #77, #36, and #369 dashboard/PnL dependencies.
4. Define freshness/staleness behavior for each live/current field.
5. Run product gate for Grafana, Metabase, Superset/Preset, Retool,
   Plotly/Dash, and custom fallback.
6. Define no-mutation controls for selected product/UI.
7. Validate read-only source contract before UI implementation.
8. Validate displayed artifact links resolve under configured S3 `artifact_root` and
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

Matrix semantics come from `../reference/contracts.md`. The authoritative
dashboard field/source contract is the `DashboardFieldSource` struct in
`crates/backtesting-vertical-slice/src/dashboard_contract.rs`.
`proof_pin_reason_code` and `proof_pin_reason_detail` are owned by
`crates/backtesting-vertical-slice/src/run_manifest.rs`.

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

Decision: select Metabase Open Source/self-hosted as the first read-only SQL BI
surface for dashboard read-model tables, with read-only warehouse credentials
and disabled mutation actions. Grafana remains an observability companion for
ops metrics/logs; Preset/Superset remains the managed/open-source BI fallback;
Retool is not selected until annotation/review write owner/schema/audit rules
exist; Plotly/Dash and bespoke UI are rejected for now because the required
first surface is tabular SQL BI plus drill-down, not a custom visual app.

Selected products or UI surfaces must carry a no-mutation-controls reference.
Non-trading annotation/review writes stay disabled unless a future owner/schema/
audit reference exists.

## Product Cost Baselines

Refreshed on 2026-06-15 from public vendor pricing pages. These are planning
snapshots; final procurement must re-check price, contract, security, and usage
limits before purchase or deployment.

| Product path | Current public pricing snapshot | Dashboard implication |
|---|---:|---|
| Grafana Cloud | Free `$0`; Pro from `$19/month + usage`; Enterprise starts at `$25k/year` spend commit. Source: https://grafana.com/pricing/ | Strong ops metrics/logs companion; not trading truth and not the first SQL investigation surface. |
| Metabase | Open Source `$0`; managed Starter `$100/month + $6/user/month`; Pro `$575/month + $12/user/month`; Enterprise starts at `$20k/year`. Source: https://www.metabase.com/pricing/ | Selected first read-only SQL BI path; use read-only DB credentials and no dashboard write actions. |
| Preset/Superset | Preset Starter `$0` up to 5 users; Professional `$20/user/month` annually or `$25/user/month` monthly; embedded viewer licenses start at `$500/month` for 50 viewers; Enterprise custom. Source: https://preset.io/pricing/ | Fallback BI path if Metabase cannot satisfy SQL/dashboard workflow or Superset compatibility wins. |
| Retool | Free tier; Team builder `$10/month` and internal user `$5/month`; Business builder `$50/month` and internal user `$15/month`; Business/Enterprise expose stronger controls such as RBAC and audit logs. Source: https://retool.com/pricing/ | Not selected for first dashboard; reconsider only if annotation/review workflow gets owner/schema/audit rules. |
| Plotly/Dash | Free `$0`; Pro `$29/creator/month` or `$290/year`; extra viewers `$10/seat/month`; Enterprise custom. Source: https://plotly.com/pricing/ | Rejected for first dashboard because current jobs are BI tables/drill-down, not bespoke custom visuals. |
| Custom UI | No product fee; engineering, security, and ops cost unbounded. | Rejected for now; allowed only by explicit custom-UI exception after product candidates fail with evidence. |

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
