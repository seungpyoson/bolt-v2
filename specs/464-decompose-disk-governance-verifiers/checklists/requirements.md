# Requirements Checklist: #464 Cargo Scanner Helper Decomposition

## Scope Quality

- [x] One issue is named: #464.
- [x] One bounded slice is named: cargo scanner helper extraction.
- [x] Non-goals are explicit in `plan.md` and `spec.md`.
- [x] Current source of truth is fresh `origin/main` after PR #461.
- [x] Remaining #464 scope is not claimed as resolved by this slice.

## Testability

- [x] Characterization-first test path is named.
- [x] RED failure cause is specified.
- [x] GREEN verification commands are specified.
- [x] Exact-head CI and external review gates are specified.

## Behavior Preservation

- [x] No new shell semantics are in scope.
- [x] No new wrapper families are in scope.
- [x] No cargo policy expansion is in scope.
- [x] Runtime target-routing policy remains local.
- [x] Static workflow target-routing policy remains local.

## Review Gate

- [x] Claude planning review slot is required.
- [x] Gemini planning review slot is required.
- [x] Grok planning review slot is required.
- [x] GLM planning review slot is required with approval metadata.
- [x] DeepSeek planning review slot is required with approval metadata.
- [x] Kimi planning review slot is required.
- [x] Missing or failed review slots do not count as approval.
