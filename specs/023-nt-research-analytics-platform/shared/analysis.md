# Analysis: NT-First Research Planning Package

Date: 2026-05-21

## Findings

| Severity | Finding | Evidence | Required action |
|---|---|---|---|
| HIGH | Provider selection cannot be made yet because fidelity, license, schema, sample, and all-in cost proofs are incomplete. Cost is now a review lever, not a reason to weaken architecture during this phase. | `shared/evidence.md` E-011, E-015, E-016, E-024; `shared/cost-model.md`; `shared/fidelity-matrix.md`. | Produce best-fidelity shortlist first, then mark over-target modes for user review and later cost cuts. |
| HIGH | Kalshi adapter readiness is a user assumption and is out of scope to prove here; Kalshi data-source and historical-fidelity proof still remains. | `shared/evidence.md` E-008, E-009; `shared/fidelity-matrix.md`. | Treat adapter as ready; prove Kalshi source contracts, schema, L2 availability or lower-fidelity class before implementation. |
| HIGH | HIP-4 live adapter support is proven upstream, but historical execution-quality data is not. | `shared/evidence.md` E-003..E-007. | Split HIP-4 lifecycle proof from HIP-4 historical data proof. |
| HIGH | Dashboard PnL completeness depends on `PortfolioSnapshot`, durable PnL/history, redemption-PnL inclusion/exclusion gates, and production-readiness non-closure context. | `shared/evidence.md` E-017, E-018, E-020; #409, #77, #36, #369. | Make #409, #77, #36, and #369 scope decision prerequisites for PnL/dashboard completeness or non-closure claims. |
| HIGH | Official venue API/archive capture was previously overclaimed. It is now a per-venue `GAP` until source-proven. | `shared/evidence.md` E-022; `shared/research.md` Provider Evidence. | Add current official source proof before selecting official capture for any venue. |
| HIGH | Venue-specific evidence examples must not become architecture. Venue/product/provider identity belongs in TOML-selected registry bindings, not hardcoded core branches. | `shared/evidence.md` E-026; `shared/contracts.md` Venue Gates; numbered project specs. | Keep provider and venue proof generic; require config/registry binding evidence before implementation. |
| HIGH | Upstream NT capability and Bolt manifest enablement are separate gates. The target `bolt-v2` implementation branch's selected NT version must be used, and required NT crates/features may still need manifest enablement proof. | `shared/evidence.md` E-027; `Cargo.toml`; `Cargo.lock`. | Prove the `bolt-v2`-selected NT version plus manifest/feature enablement before implementation; do not create Bolt-owned duplicate engines or adapters. |
| MEDIUM | SpecKit default resolution is branch-local state. This branch now points `.specify/feature.json` at 023; before that pointer update, default scripts resolved the existing 014 package even on the 023 branch. | `.specify/feature.json`; `.specify/scripts/bash/check-prerequisites.sh --json --require-tasks --include-tasks`; `.specify/scripts/bash/setup-tasks.sh --json`. | Keep the 023 feature pointer in this branch and verify default SpecKit scripts before implementation or issue mutation. |
| MEDIUM | Review feedback identified draft overclaims in perpetual-futures venue assumptions, official venue capture, issue mapping, release labels, status vocabulary, external-review sourcing, dashboard dependency mapping, and unsafe parallel task markers. | Review feedback is challenge input only; accepted fixes are recorded in shared root artifacts and the numbered project directories. | Re-run if issue payloads change materially; do not cite reviewer agreement as source proof. |
| MEDIUM | Follow-up review scope must include shared root artifacts plus the exact numbered project directory under review. | Root `tasks.md`; `1-backtesting-engine/`, `2-research-analytics/`, and `3-dashboard/`. | Re-run focused review if issue payloads or provider selections change materially. |
| MEDIUM | Dashboard product choice is not yet selected. Existing BI/observability products are source-identified, but source-contract, security, query-backend, UX, and all-in cost proof remain open. | `shared/evidence.md` E-028; `shared/cost-model.md`; `3-dashboard/plan.md`. | Evaluate Grafana, Metabase, Preset/Superset, Retool, Plotly/Dash, and bespoke UI fallback before Dashboard implementation planning is accepted. |

## Consistency Checks

- Root `spec.md` is a Speckit compatibility shim, not a single implementation spec.
- Root `plan.md` is a Speckit compatibility shim and triage pointer, not a
  single implementation plan.
- Numbered project specs/plans own project-specific requirements and tasks.
- `shared/research.md` no longer treats external-review agreement as evidence.
- `shared/evidence.md` treats perpetual-futures venues through generic venue gates, official venue capture as per-venue `GAP`, and expanded issue evidence.
- `shared/cost-model.md` records current provider-cost facts, over-target review status, and blocking unknowns.
- `shared/fidelity-matrix.md` classifies venue/source families without hardcoding venue-specific architecture.
- Root `tasks.md` only links shared package tasks and the three project task lists.
- `shared/contracts.md` prohibits overclaims for Kalshi, HIP-4 history, provider selection, official venue capture, dashboard PnL completeness, and hardcoded venue identity.
- `shared/data-model.md` contains shared entities only; project-specific models live in numbered project docs.

## Review Run Status

- Earlier review output is now treated as stale challenge input because this
  package was re-scoped on 2026-05-21 from one "research analytics platform"
  into three future vertical specs/plans.
- Read-only background review on 2026-05-21 challenged the Backtesting Engine,
  Research Analytics, and Dashboard slices separately. Accepted findings are now
  split into `1-backtesting-engine/`, `2-research-analytics/`, `3-dashboard/`,
  and shared root artifacts.
- Local cross-artifact sweep after the latest edits found no stale
  single-platform title, stale NT pointer, stale main pointer, runtime-build task, or
  issue-mutation task in the 023 package.
- Follow-up review found traceability defects in the earlier umbrella
  Backtesting/Analytics/Dashboard requirement trace, staged issue evidence rows,
  deliverables wording, issue-state freshness date, and `ExperimentRun` entity
  coverage. The package is now decomposed into numbered project directories plus
  shared root evidence.
- Reviewer agreement is not source proof; accepted findings must appear as
  concrete artifact changes and evidence rows.

## Residual Risk

- No code was implemented or compiled.
- No NT pointer update was attempted.
- No GitHub issues were created or edited.
- No DeepSeek/GLM direct API review was run after the user said it was not
  needed.
- Branch-local `.specify/feature.json` now points at this 023 package, so default
  SpecKit scripts can resolve it from this branch.
- Explicit read-only override also works for this package:
  `SPECIFY_FEATURE=023-nt-research-analytics-platform
  SPECIFY_FEATURE_DIRECTORY=specs/023-nt-research-analytics-platform`.
  `setup-tasks.sh --json` also resolves the 023 feature directory and template,
  and `check-prerequisites.sh --paths-only` reports the 023 spec, plan, and
  tasks paths. Main previously resolved 014 or failed branch naming; this branch
  is the reviewable SpecKit surface for 023.
- Web pricing/vendor docs can drift; cost model must refresh them at the time of selection.
