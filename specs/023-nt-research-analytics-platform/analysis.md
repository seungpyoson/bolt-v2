# Analysis: NT-First Research Analytics Platform

Date: 2026-05-20

## Findings

| Severity | Finding | Evidence | Required action |
|---|---|---|---|
| BLOCKER | Provider selection cannot be made yet. Tardis Professional is `$900/month`; draft cost model shows this leaves <= `$100/month` for AWS/dashboard unless waived. | `evidence.md` E-011; `cost-model.md`; `tasks.md` T013. | Prove AWS/dashboard reserve or request waiver before selecting Tardis. |
| BLOCKER | Kalshi adapter support is intentionally a user assumption, not source-proven in the checked NT clone. | `evidence.md` E-008; `tasks.md` T010. | Keep planning from supported-adapter premise, but require exact source/pointer proof in the Kalshi issue. |
| HIGH | HIP-4 live adapter support is proven upstream, but historical execution-quality data is not. | `evidence.md` E-003..E-007. | Split HIP-4 lifecycle proof from HIP-4 historical data proof. |
| HIGH | Dashboard PnL completeness depends on `PortfolioSnapshot`, durable PnL/history, redemption-PnL inclusion/exclusion gates, and production-readiness non-closure context. | `evidence.md` E-017, E-018, E-020; #409, #77, #36, #369. | Make #409, #77, #36, and #369 scope decision prerequisites for PnL/dashboard completeness or non-closure claims. |
| HIGH | Official venue API/archive capture was previously overclaimed. It is now a per-venue `GAP` until source-proven. | `evidence.md` E-022; `research.md` Provider Evidence. | Add current official source proof before selecting official capture for any venue. |
| HIGH | Venue-specific evidence examples must not become architecture. Venue/product/provider identity belongs in TOML-selected registry bindings, not hardcoded core branches. | `evidence.md` E-026; `contracts/nt-research-analytics.md` Venue Gates; `spec.md` FR-016. | Keep provider and venue proof generic; require config/registry binding evidence before implementation. |
| HIGH | Upstream NT capability and Bolt manifest enablement are separate gates. Current Bolt pin is verified, but `nautilus-backtest`, `nautilus-hyperliquid`, and `nautilus-tardis` are not all direct Bolt dependencies today. | `evidence.md` E-027; `Cargo.toml`; `Cargo.lock`; local NT checkout `7c2aafb30fb143069c915a3f2057bb12174405f6`. | Prove selected pointer plus manifest/feature enablement before implementation; do not create Bolt-owned duplicate engines or adapters. |
| MEDIUM | SpecKit default resolution is branch-local state. This branch now points `.specify/feature.json` at 023; before that pointer update, default scripts resolved the existing 014 package even on the 023 branch. | `.specify/feature.json`; `.specify/scripts/bash/check-prerequisites.sh --json --require-tasks --include-tasks`; `.specify/scripts/bash/setup-tasks.sh --json`. | Keep the 023 feature pointer in this branch and verify default SpecKit scripts before implementation or issue mutation. |
| MEDIUM | Review feedback identified draft overclaims in perpetual-futures venue assumptions, official venue capture, issue mapping, release labels, status vocabulary, external-review sourcing, dashboard dependency mapping, and unsafe parallel task markers. | Review feedback is challenge input only; accepted fixes are recorded in source artifacts: `evidence.md`, `research.md`, `plan.md`, `contracts/nt-research-analytics.md`, `cost-model.md`, `fidelity-matrix.md`, `tasks.md`, `github-issues.md`, and `data-model.md`. | Re-run if issue payloads change materially; do not cite reviewer agreement as source proof. |
| MEDIUM | External review scope must include all controlling artifacts, not only the six files sent to the first provider quorum. | `tasks.md` T039 now names the constitution, spec, plan, research, evidence, cost model, fidelity matrix, data model, contract, analysis, tasks, issue payloads, issue audit, and checklist. | Run the expanded review package before issue creation or implementation. |
| MEDIUM | Dashboard product choice is not yet selected. Existing BI/observability products are now source-identified, but source-contract, security, query-backend, UX, and all-in cost proof remain open. | `evidence.md` E-028; `cost-model.md`; `tasks.md` T030. | Evaluate Grafana, Metabase, Preset/Superset, Plotly/Dash, and bespoke UI fallback before dashboard MVP work. |

## Consistency Checks

- `spec.md` now states `evidence.md` controls requirements.
- `plan.md` now lists `evidence.md` as the controlling source and treats Kalshi as `USER_ASSUMPTION`.
- `research.md` no longer treats external-review agreement as evidence.
- `evidence.md` now treats perpetual-futures venues through generic venue gates, official venue capture as per-venue `GAP`, and expanded issue evidence.
- `cost-model.md` records current provider-cost facts, cap status, and blocking unknowns.
- `fidelity-matrix.md` classifies venue/source families without hardcoding venue-specific architecture.
- `tasks.md` is now SpecKit-style: task IDs, story labels only in user-story phases, paths in each row, dependencies, parallel examples, and MVP slice.
- `contracts/nt-research-analytics.md` prohibits overclaims for Kalshi, HIP-4 history, Tardis selection, official venue capture, dashboard PnL completeness, and hardcoded venue identity.
- `data-model.md`, `analysis.md`, and `research.md` exist in this package and are included in the expanded T039 review scope.

## Review Run Status

- Expanded review packet on 2026-05-20 covered the constitution, spec, plan,
  research, evidence, cost model, fidelity matrix, data model, contract,
  analysis, tasks, issue payloads, issue audit, and checklist.
- GLM, DeepSeek, Gemini, Grok, and GPT-subagent produced usable review output.
- Claude source was not sent because OAuth inference failed before launch.
- Kimi source was sent, but the slot failed review-quality audit and is not
  counted as usable approval.
- Accepted review findings were translated into artifact changes; reviewer
  agreement is not used as source proof.

## Residual Risk

- No code was implemented or compiled.
- No NT pointer update was attempted.
- No GitHub issues were created or edited.
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
