# Research: #344 Residual Minute-Consumption Work

## Decision: Keep #335 paths-ignore intact

**Rationale**: #335 is closed with narrowed acceptance: `pull_request.paths-ignore` only, no `push` filtering. #344 explicitly says it owns no change to `.github/workflows/ci.yml` paths-ignore behavior.

**Rejected alternative**: Replace workflow-level `paths-ignore` with job-level skips. That could solve pending required checks differently, but it would reopen #335 and change accepted scope.

## Decision: Pass-stub must inspect actual changed files

**Rationale**: GitHub path filters alone cannot express "only ignored-safe files changed" for mixed PRs. The pass-stub must compute changed files and classify them against the same safe patterns as CI `paths-ignore`.

**Rejected alternative**: Trust workflow `paths` patterns alone. That can trigger on mixed safe+source changes and cannot prove docs-only eligibility.

## Decision: Preserve real CI gate for source and mixed changes

**Rationale**: #333 requires no merge-gate weakening. Mixed docs+source PRs must remain full-CI cases. The pass-stub is only compatibility for ignored-safe PRs where the main CI workflow is intentionally skipped.

## Primary Sources

- GitHub docs: workflows skipped by branch/path filtering can leave required checks pending.
- GitHub docs: jobs skipped by condition report success.
- Live branch protection: `main` currently has `required_status_checks: null`.
