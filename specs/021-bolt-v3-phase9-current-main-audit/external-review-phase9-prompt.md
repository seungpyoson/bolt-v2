# External Review Prompt: PR #331 P9 Audit

Review PR #331 Phase 9 audit artifacts for `bolt-v2` / bolt-v3.

The orchestrator must inject the live PR head and base from:

```bash
gh pr view 331 --json headRefOid,baseRefOid,mergeStateStatus,state
```

Scope:

- `specs/021-bolt-v3-phase9-current-main-audit/spec.md`
- `specs/021-bolt-v3-phase9-current-main-audit/checklists/requirements.md`
- `specs/021-bolt-v3-phase9-current-main-audit/plan.md`
- `specs/021-bolt-v3-phase9-current-main-audit/research.md`
- `specs/021-bolt-v3-phase9-current-main-audit/data-model.md`
- `specs/021-bolt-v3-phase9-current-main-audit/contracts/audit-evidence.md`
- `specs/021-bolt-v3-phase9-current-main-audit/quickstart.md`
- `specs/021-bolt-v3-phase9-current-main-audit/audit-report.md`
- `specs/021-bolt-v3-phase9-current-main-audit/ai-slop-cleanup-report.md`
- `specs/021-bolt-v3-phase9-current-main-audit/tasks.md`
- `specs/021-bolt-v3-phase9-current-main-audit/external-review-phase9-disposition.md`
- `specs/021-bolt-v3-phase9-current-main-audit/external-review-phase9-prompt.md`
- `specs/021-bolt-v3-phase9-current-main-audit/external-review-phase9-relay-prompts.md`
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md`
- `docs/bolt-v3/2026-05-18-production-readiness-contract.md`
- `specs/001-thin-live-canary-path/tasks.md`
- `config/root.toml`
- `config/strategies/binary_oracle.example.toml`

Hard constraints:

- main is authoritative after merge; this review is PR #331 exact-head only
- no live capital, no soak, no real order, no deploy
- no secret display
- bolt-v3 stays thin over NautilusTrader
- NT owns lifecycle, reconciliation, cache, adapter behavior, and order machinery
- SSM is the only secret source
- pure Rust runtime, no Python runtime layer
- no hardcoded runtime values
- no dual readiness or submit paths
- no broad "live ready" or "production ready" claim without claim-level evidence
- PR #392 scope is downstream and must not be implemented inside PR #331

Questions:

1. Are the P9 blockers complete and correctly classified for the current exact head?
2. Does the audit correctly distinguish P7/P8 source-review closure from unrun live evidence?
3. Does the audit avoid treating tracked example TOML as active operator config?
4. Does the audit preserve the NT boundary and SSM-only boundary?
5. Does the audit correctly block no-submit live readiness, tiny-canary completion, staged live, and production live claims?
6. Are retired path/head/PR references still present as current evidence in P9 artifacts?
7. Are any FR-003 audit categories missing or overclaimed?
8. Is the exact-head review evidence strategy sound, given committed docs cannot safely hardcode their own final commit SHA?

Return:

- First line exactly: `Verdict: APPROVE`, `Verdict: REQUEST_CHANGES`, or `Verdict: NEEDS_INFO`
- severity-ranked findings with file/line evidence
- explicit statement whether P9 source-review closure may proceed after local verification and CI
