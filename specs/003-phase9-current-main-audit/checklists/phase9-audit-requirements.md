# Phase 9 Audit Requirements Checklist

**Purpose**: Validate the Phase 9 current-main audit requirements and evidence coverage.
**Created**: 2026-05-14
**Feature**: [spec.md](../spec.md)

## Current-main Provenance

- [x] CHK001 Audit names branch `022-bolt-v3-phase9-current-main-audit`.
- [x] CHK002 Audit records original source anchor `23acab30b73990302765ea441550fabcbf03f570` and final refreshed `origin/main` base `fde50d3452859a51f7f27b807913b1f12697b273`.
- [x] CHK003 Audit treats stale `019` and scratch `021` as reference-only.

## Coverage Completeness

- [x] CHK004 Audit covers `src/bolt_v3_*.rs`.
- [x] CHK005 Audit covers `src/bolt_v3_*/**/*.rs`.
- [x] CHK006 Audit covers required runtime-used shared paths.
- [x] CHK007 Audit covers bolt-v3 tests and fixtures.
- [x] CHK008 Audit covers verifier scripts.
- [x] CHK009 Audit covers roadmap docs and prior specs.
- [x] CHK026 Audit includes explicit follow-up dispositions for archetypes, provider registry, Binance provider, client registration, strategy registration, live canary gate, submit admission, and tiny-canary evidence.

## Required Proof Commands

- [x] CHK010 Literal coverage command is recorded.
- [x] CHK011 Policy coverage command is recorded.
- [x] CHK012 Verifier inspection is recorded.
- [x] CHK013 Roadmap/status-doc inspection is recorded.

## Classification Quality

- [x] CHK014 Runtime literals are classified by ownership category.
- [x] CHK015 Policy hardcodes and fail-closed scope constraints are separated from protocol labels.
- [x] CHK016 Dual-path and legacy default surfaces are called out.
- [x] CHK017 Debt and AI-slop marker scan is recorded.
- [x] CHK018 SSM-only and pure Rust runtime evidence is recorded without exposing secrets.
- [x] CHK019 Strategy math and feed assumptions are bounded to config/source evidence.

## Runtime-capture Concern

- [x] CHK020 `run_bolt_v3_live_node` runtime-capture failure path is classified as real bug, false positive, or needs test.
- [x] CHK021 Any proposed runtime-capture implementation is approval-gated.

## Live-action Gate

- [x] CHK022 Audit excludes live capital, soak, merge, runtime cleanup, and source-bearing external review.
- [x] CHK023 Findings are severity-ranked.
- [x] CHK024 Cleanup candidates include behavior locks.
- [x] CHK025 Decision outcome is explicit.

## External Review Gate

- [x] CHK027 Draft PR #331 exists for Phase 9 audit/remediation and is not merged.
- [x] CHK028 Exact-head PR CI is green before external reviews.
- [x] CHK029 Claude review completed, but is classified as docs-only branch-diff coverage.
- [x] CHK030 Gemini review completed, but is classified as docs-only branch-diff coverage.
- [x] CHK031 DeepSeek full-scope shard review is complete and dispositions are recorded.
- [x] CHK032 GLM full-scope shard review is complete and dispositions are recorded.
- [x] CHK033 Gemini Code Assist PR comments are reviewed and dispositions are recorded.
- [x] CHK034 Greptile PR/comment/check surfaces are checked; actionable Greptile P2 output is fixed and dispositioned.
- [x] CHK035 Oversized TOML reads fail closed before parsing for legacy and Bolt-v3 config loaders.
- [x] CHK036 Expired fair-probability computation fails closed with `None`.
- [x] CHK037 AI-slop marker scan evidence is rerun and recorded.
- [x] CHK038 T034/T039/T040/T060/T061/T062/T063 moved from re-acceptance or classification to implementation; Gemini `46e1d661-5001-4f76-9f5b-367df876626d`, Claude `d1746da7-27d0-408b-8446-cce186e895df`, DeepSeek `job_31d3e8d9-5e75-436e-b4b2-60b193c9a30b`, and GLM `job_7f550722-951f-4d8a-a0a6-85a011f7855f` post-implementation review approved with no blockers at `b897dd6`.
- [x] CHK045 Strategy outcome-side inference no longer parses hardcoded `-UP.`/`-DOWN.` instrument-id suffixes; red/green, local batch verification, exact-head CI at `bf2ad6f`, four-provider follow-up external review, `535f973` exact-head CI, and final narrow DeepSeek/GLM review pass.
- [x] CHK046 Legacy live-local instrument-id validation no longer pins `.POLYMARKET`; red/green, local batch verification, exact-head CI at `bf2ad6f`, four-provider follow-up external review, Claude/GLM venue-test gap closure, `535f973` exact-head CI, and final narrow DeepSeek/GLM review pass.
- [x] CHK047 Bolt-v3 adapter mapping no longer installs a `0_i64` clock sentinel; red/green, local batch verification, exact-head CI at `bf2ad6f`, four-provider follow-up external review, `535f973` exact-head CI, and final narrow DeepSeek/GLM review pass.
- [x] CHK039 Final branch refresh to current `origin/main` is recorded and the upstream delta is classified as workflow-maintenance-only.
- [x] CHK040 `no-mistakes` is run on the Phase 9 head and every substantive finding is fixed or dispositioned.
- [x] CHK041 Active Bolt-v3 schema docs/examples include required Polymarket `auto_load_debounce_milliseconds`.
- [x] CHK042 Generated live-config materialization rewrites oversized or invalid drifted output instead of failing before repair.
- [x] CHK043 Pure-Rust runtime verifier includes runtime-capture and strategy modules.
- [x] CHK044 The 1 MiB pre-parse operator-config size guard is explicitly documented as a resource-exhaustion guard, not trading policy.
