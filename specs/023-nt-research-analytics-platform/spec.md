# Spec: NT Research Planning Package

This root file is a Speckit compatibility shim. It is not an implementation
specification.

Use the numbered project directories:

1. [Backtesting Engine](1-backtesting-engine/spec.md)
2. [Research Analytics](2-research-analytics/spec.md)
3. [Dashboard](3-dashboard/spec.md)

Human triage starts at [README.md](README.md).

Shared cross-project authority remains in `shared/evidence.md`,
`shared/data-model.md`, and `shared/contracts.md`. The numbered project docs
inherit those files and expose project-specific derived views, requirements,
architecture, tasks, and acceptance criteria.

Other `shared/` files preserve audit and planning history: cost/fidelity
archives, issue payload drafts, issue-search evidence, review notes, and
checklist history.

No runtime implementation, GitHub issue mutation, provider recorder, dashboard
UI, backtesting runner, or analytics implementation is authorized by this root
file.
