# Implementation Plan: #344 Residual Minute-Consumption Work

**Branch**: `codex/ci-344-residual-minute-work` | **Date**: 2026-05-15 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/009-ci-residual-minute-work/spec.md`

## Summary

Implement the unblocked #344 slice: docs for current path-filter behavior, a fail-closed changed-file classifier, a pass-stub workflow for future required-check compatibility, verifier coverage, and branch hygiene evidence. Keep #335 `paths-ignore` intact and mark real docs-only PR evidence plus post-stack minute rebaseline as blocked until the stack can produce real runs.

## Technical Context

**Language/Version**: Python 3.12-compatible stdlib, GitHub Actions YAML
**Primary Dependencies**: existing CI workflow, GitHub pull_request metadata, git diff path listing
**Storage**: Markdown docs/specs and GitHub issue comments
**Testing**: Python verifier self-tests, workflow verifier, YAML parse, `just ci-lint-workflow`
**Target Platform**: GitHub Actions `ubuntu-latest`
**Project Type**: CI workflow/docs/tooling
**Performance Goals**: Avoid future pending required checks for ignored-safe PRs with only a tiny classifier workflow
**Constraints**: no destructive branch cleanup, no #340 relocation, no post-stack rebaseline claim, no weakening PR/main/tag CI
**Scale/Scope**: One #344 slice stacked on #205

## Constitution Check

- **NO HARDCODES**: PASS. CI path patterns are workflow contract literals; Rust runtime config untouched.
- **NO DUAL PATHS**: PASS. Classifier reads one safe path set matching CI `paths-ignore`.
- **NO DEBTS**: PASS. Blocked evidence is named in tasks/PR/issue, not claimed complete.
- **NO CREDENTIAL DISPLAY**: PASS. No secrets displayed.
- **PURE RUST BINARY**: PASS. No runtime layer change.
- **SSM SINGLE SECRET SOURCE**: PASS. Secret flow unchanged.
- **GROUP BY CHANGE**: PASS. #344 residual docs/pass-stub/branch inventory only.
- **DO NOT REFERENCE BOLT V1**: PASS.

## Project Structure

```text
docs/ci/paths-ignore-behavior.md
docs/ci/branch-hygiene-2026-05-15.md
.github/workflows/ci-docs-pass-stub.yml
scripts/verify_ci_path_filters.py
scripts/test_verify_ci_path_filters.py
scripts/verify_ci_workflow_hygiene.py
scripts/test_verify_ci_workflow_hygiene.py
specs/009-ci-residual-minute-work/
```

## Stage 0 - Evidence And Research

- Re-read #344, #335, #333, #340 live state.
- Confirm main branch has no required status checks today.
- Confirm current branch list before any classification.
- Confirm GitHub skipped-workflow / skipped-job semantics from primary docs.

## Stage 1 - Design

- Represent safe path patterns once in a verifier/classifier.
- Add docs table covering required #344 examples.
- Add pass-stub workflow that computes changed files and exposes `gate` only for ignored-safe PRs.
- Extend workflow lint to guard pass-stub and docs drift.

## Stage 2 - Implementation

- Add path-filter classifier tests first.
- Implement classifier/verifier and docs.
- Add pass-stub workflow.
- Extend `just ci-lint-workflow`.
- Generate branch hygiene artifact/comment.

## Stage 3 - Verification And Handoff

- Run local verifier stack and YAML parse.
- Push draft PR stacked on #350.
- Comment on #344/#333 with completed vs blocked evidence.
