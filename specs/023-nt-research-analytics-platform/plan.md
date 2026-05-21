# Plan: NT Research Planning Package

This root file is a Speckit compatibility shim and triage pointer. It is not a
project implementation plan.

## Triage Order

1. Pick exactly one numbered project directory.
   If no vertical is explicitly selected, default to `1-backtesting-engine/`.
2. Review that project's `spec.md`, `plan.md`, and `tasks.md`.
3. Consult `reference/` for authoritative cross-project evidence/contracts/data
   models or for original evidence rows, issue-audit details, staged issue
   payloads, cost/fidelity archives, or review/question/checklist history.
4. For Issues B-D, open or update only the selected project payload; handle
   cross-project/process payloads A and E only with explicit user approval.
5. Start implementation only in a fresh future session for that vertical.

## Project Directories

| Directory | Entry Points |
|---|---|
| `1-backtesting-engine/` | `spec.md`, `plan.md`, `tasks.md` |
| `2-research-analytics/` | `spec.md`, `plan.md`, `tasks.md` |
| `3-dashboard/` | `spec.md`, `plan.md`, `tasks.md` |

Root artifacts do not authorize runtime code or cross-project implementation
scope.
