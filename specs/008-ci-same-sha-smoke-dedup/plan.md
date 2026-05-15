# Implementation Plan: #205 Same-SHA Smoke-Tag Dedup

**Branch**: `codex/ci-205-same-sha-smoke-dedup` | **Date**: 2026-05-15 | **Spec**: [spec.md](spec.md)  
**Input**: Feature specification from `specs/008-ci-same-sha-smoke-dedup/spec.md`

## Summary

Implement a fail-closed same-SHA evidence resolver for tag workflows and wire the CI topology so smoke tags reuse the exact successful `main` CI run artifact instead of rerunning duplicate heavy lanes. Preserve full PR and `main` push CI behavior, keep deploy behind the aggregate `gate`, and record that issue closure still requires real after-evidence from a post-merge smoke tag.

## Technical Context

**Language/Version**: Python 3.12-compatible stdlib for verifier/resolver; GitHub Actions YAML  
**Primary Dependencies**: GitHub Actions REST API, `actions/download-artifact` cross-run artifact inputs, existing `just ci-lint-workflow` verifier path  
**Storage**: GitHub Actions run/job/artifact metadata and workflow outputs only  
**Testing**: Python self-tests, workflow verifier, YAML parse, `just ci-lint-workflow`  
**Target Platform**: GitHub Actions `ubuntu-latest`  
**Project Type**: CI workflow topology and repository scripts  
**Performance Goals**: Remove duplicate tag-run `test` and `build` work when an exact green `main` run already exists  
**Constraints**: fail closed, exact SHA, trusted `main` run only, no PR CI weakening, no fallback rebuild, no merge without approval  
**Scale/Scope**: One #205 slice stacked on #195/#332/#203/#342 workflow shape

## Constitution Check

- **NO HARDCODES**: PASS. Workflow IDs and artifact names are CI contract constants required by issue #205; Rust runtime values remain untouched.
- **NO DUAL PATHS**: PASS. Tag deploy artifact source becomes one path: exact source run artifact ID.
- **NO DEBTS**: PASS. Pending real smoke-tag evidence is a closure blocker, not hidden implementation debt.
- **NO CREDENTIAL DISPLAY**: PASS. The resolver uses `GITHUB_TOKEN`; logs include only run/check/artifact IDs and SHA.
- **PURE RUST BINARY**: PASS. No runtime Rust or Python binary layer change.
- **SSM SINGLE SECRET SOURCE**: PASS. Deploy AWS credential path remains unchanged.
- **GROUP BY CHANGE**: PASS. #205 touches tag reuse topology only.
- **DO NOT REFERENCE BOLT V1**: PASS. No old repo dependency.

## Project Structure

```text
specs/008-ci-same-sha-smoke-dedup/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── tasks.md
└── checklists/
    └── requirements.md

.github/workflows/ci.yml
scripts/find_same_sha_main_evidence.py
scripts/test_find_same_sha_main_evidence.py
scripts/verify_ci_workflow_hygiene.py
scripts/test_verify_ci_workflow_hygiene.py
justfile
```

## Stage 0 - Evidence And Research

- Re-read live #205 and #333 requirements and comments.
- Confirm historical duplicate-run evidence from #205.
- Confirm GitHub Actions APIs can query successful `main` runs by SHA and fetch jobs/artifacts.
- Confirm `actions/download-artifact` supports `artifact-ids`, `github-token`, `repository`, and `run-id` for another workflow run.

## Stage 1 - Design

- Add a `SameShaMainEvidence` data model and resolver script.
- Add a tag-only `same-sha-main-evidence` job.
- Skip duplicate heavy lanes on tag runs.
- Teach `gate` to distinguish tag reuse mode from normal PR/main mode.
- Teach `deploy` to download the artifact by source artifact ID and log exact reused evidence.

## Stage 2 - Implementation

- Add resolver self-tests before implementation.
- Implement resolver selection, job validation, artifact validation, output writing, and GitHub API fetching.
- Extend workflow hygiene verifier and self-tests for #205 topology.
- Update CI workflow and `just ci-lint-workflow`.

## Stage 3 - Verification And Handoff

- Run resolver self-tests, workflow verifier self-tests, live verifier, YAML parse, `just ci-lint-workflow`, formatting/diff checks.
- Push draft PR.
- Update PR and issue comments with exact verification plus blocker: real after-evidence requires a merged stack and smoke tag.
