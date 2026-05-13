# External Review Prompt: Phase 9 Audit Plan

Review the Phase 9 audit artifacts for `bolt-v2` / bolt-v3.

Scope:

- `specs/019-bolt-v3-phase9-audit-fresh/spec.md`
- `specs/019-bolt-v3-phase9-audit-fresh/checklists/requirements.md`
- `specs/019-bolt-v3-phase9-audit-fresh/plan.md`
- `specs/019-bolt-v3-phase9-audit-fresh/research.md`
- `specs/019-bolt-v3-phase9-audit-fresh/data-model.md`
- `specs/019-bolt-v3-phase9-audit-fresh/contracts/audit-evidence.md`
- `specs/019-bolt-v3-phase9-audit-fresh/quickstart.md`
- `specs/019-bolt-v3-phase9-audit-fresh/audit-report.md`
- `specs/019-bolt-v3-phase9-audit-fresh/ai-slop-cleanup-report.md`
- `specs/019-bolt-v3-phase9-audit-fresh/tasks.md`

Hard constraints:

- main is source of truth
- no stale branch continuation
- no live capital, no soak, no real order
- Bolt-v3 must stay thin over NautilusTrader
- NT owns lifecycle, reconciliation, cache, adapter behavior, order machinery
- SSM is the only secret source
- pure Rust runtime, no Python runtime layer
- no hardcoded runtime values
- no dual paths
- no debt-marker cleanup without behavior tests

Questions:

1. Are the blockers complete and correctly classified?
2. Does the plan avoid treating stale Phase 7/8 branch work as accepted scope?
3. Does the plan preserve the NT boundary?
4. Does the plan correctly block Phase 8 live action on unresolved strategy-input safety and live-ops readiness?
5. Are cleanup rules strict enough to prevent untested refactor drift?
6. Are any required audit categories missing?

Return:

- APPROVE or REQUEST_CHANGES
- severity-ranked findings with file/line evidence
- explicit statement whether implementation may proceed after local verification
