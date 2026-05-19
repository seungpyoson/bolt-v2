# Phase 9 External Review Relay Prompts

Status: retained source-free fallback only. Direct plugin/API reviews are the preferred current path.

Purpose: provide manual relay text only if a reviewer plugin/API path is unavailable. The operator must supply selected files through an approved channel and record returned findings in PR #331 evidence comments.

Before use:

1. Fetch live PR #331 metadata:
   `gh pr view 331 --json headRefOid,baseRefOid,mergeStateStatus,state`
2. Confirm local `git rev-parse HEAD` equals PR `headRefOid`.
3. Confirm exact-head CI is green.
4. Attach or provide the selected files listed below.
5. Do not treat this relay prompt as satisfying the review gate until returned findings are recorded.

## Selected Files

- `specs/021-bolt-v3-phase9-current-main-audit/spec.md`
- `specs/021-bolt-v3-phase9-current-main-audit/checklists/requirements.md`
- `specs/021-bolt-v3-phase9-current-main-audit/plan.md`
- `specs/021-bolt-v3-phase9-current-main-audit/research.md`
- `specs/021-bolt-v3-phase9-current-main-audit/data-model.md`
- `specs/021-bolt-v3-phase9-current-main-audit/contracts/audit-evidence.md`
- `specs/021-bolt-v3-phase9-current-main-audit/quickstart.md`
- `specs/021-bolt-v3-phase9-current-main-audit/audit-report.md`
- `specs/021-bolt-v3-phase9-current-main-audit/ai-slop-cleanup-report.md`
- `specs/021-bolt-v3-phase9-current-main-audit/external-review-phase9-prompt.md`
- `specs/021-bolt-v3-phase9-current-main-audit/tasks.md`
- `specs/021-bolt-v3-phase9-current-main-audit/external-review-phase9-disposition.md`
- `specs/021-bolt-v3-phase9-current-main-audit/external-review-phase9-relay-prompts.md`
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md`
- `docs/bolt-v3/2026-05-18-production-readiness-contract.md`
- `specs/001-thin-live-canary-path/tasks.md`
- `config/root.example.toml`
- `config/strategies/binary_oracle.example.toml`

## Relay Prompt

Review PR #331 Phase 9 audit artifacts on the exact head and base supplied by the operator.

This is documentation and audit evidence state only; do not propose runtime code edits unless you identify a source-backed blocker that invalidates the audit. Verify the artifacts:

- satisfy the Phase 9 audit requirements
- remain source-grounded
- distinguish P7/P8 source-review closure from unrun live evidence
- keep no-submit live readiness, tiny-canary completion, staged live, and production live blocked until their exact evidence exists
- avoid authorizing soak, deploy, or live capital
- keep PR #392 downstream
- do not carry retired path/head/PR references as current evidence

Return first line exactly as `Verdict: APPROVE`, `Verdict: REQUEST_CHANGES`, or `Verdict: NEEDS_INFO`. Then list blocking findings first, then non-blocking findings, with file and line evidence. If no blocking findings exist, say that explicitly and list residual risks.
