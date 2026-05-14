# Phase 9 External Review Relay Prompts

Status: source-free handoff only. This file is not external-review evidence.

Purpose: provide relay text for DeepSeek and GLM if direct API source
transmission is not approved. The operator must supply the selected files to the
reviewer through an approved channel and record the returned findings in
`external-review-phase9-disposition.md`.

Before use:

1. Replace `<EXACT_PR_HEAD>` with the current PR head SHA.
2. Confirm the base is `origin/main` at
   `d6f55774c32b71a242dcf78b8292a7f9e537afab`, or update this file before use.
3. Attach or otherwise provide the selected Phase 9 files listed below.
4. Do not treat this relay prompt as satisfying `FR-008`; only a completed
   DeepSeek or GLM review, or an explicit user waiver, satisfies that gate.

## Selected Files

- `specs/019-bolt-v3-phase9-audit-fresh/spec.md`
- `specs/019-bolt-v3-phase9-audit-fresh/checklists/requirements.md`
- `specs/019-bolt-v3-phase9-audit-fresh/plan.md`
- `specs/019-bolt-v3-phase9-audit-fresh/research.md`
- `specs/019-bolt-v3-phase9-audit-fresh/data-model.md`
- `specs/019-bolt-v3-phase9-audit-fresh/contracts/audit-evidence.md`
- `specs/019-bolt-v3-phase9-audit-fresh/quickstart.md`
- `specs/019-bolt-v3-phase9-audit-fresh/audit-report.md`
- `specs/019-bolt-v3-phase9-audit-fresh/ai-slop-cleanup-report.md`
- `specs/019-bolt-v3-phase9-audit-fresh/external-review-phase9-prompt.md`
- `specs/019-bolt-v3-phase9-audit-fresh/tasks.md`
- `specs/019-bolt-v3-phase9-audit-fresh/external-review-phase9-disposition.md`
- `specs/019-bolt-v3-phase9-audit-fresh/external-review-phase9-relay-prompts.md`

## DeepSeek Relay Prompt

Review PR #327 Phase 9 comprehensive audit planning slice on exact head
`<EXACT_PR_HEAD>` against `origin/main`
`d6f55774c32b71a242dcf78b8292a7f9e537afab`.

This is documentation and audit planning state only; do not propose code edits.
Verify the artifacts satisfy the Phase 9 audit requirements, remain
source-grounded, keep final live readiness blocked until Phase 7 and Phase 8
prerequisites are accepted on main, and do not authorize soak or live capital.
Check that Gemini and Claude review-response items are addressed and that the
external-review disposition accurately records unresolved DeepSeek and GLM
approval gating.

Return blocking findings first, then non-blocking findings, with file and line
evidence. If no blocking findings exist, say that explicitly and list residual
risks.

## GLM Relay Prompt

Review PR #327 Phase 9 comprehensive audit planning slice on exact head
`<EXACT_PR_HEAD>` against `origin/main`
`d6f55774c32b71a242dcf78b8292a7f9e537afab`.

This is documentation and audit planning state only; do not propose code edits.
Verify the artifacts satisfy the Phase 9 audit requirements, remain
source-grounded, keep final live readiness blocked until Phase 7 and Phase 8
prerequisites are accepted on main, and do not authorize soak or live capital.
Check that Gemini and Claude review-response items are addressed and that the
external-review disposition accurately records unresolved DeepSeek and GLM
approval gating.

Return blocking findings first, then non-blocking findings, with file and line
evidence. If no blocking findings exist, say that explicitly and list residual
risks.
