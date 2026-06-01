# #517 Phase 1 External Plan Review

## Review Packet

- Scope: custom review of Phase 1 plan and relevant source touch points.
- Files: `goals/production-kill-switch/phase-1-tdd-plan.md`, `goals/production-kill-switch/design.md`, `goals/production-kill-switch/issue-draft.md`, `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`, `src/bolt_v3_submit_admission.rs`, `src/lib.rs`, `Cargo.toml`.
- Packet size: 8 files, 161,508 bytes, 3,705 lines.
- Base/head reviewed: `2938bc6f6e7553e436f074163a9e5db8b4c56b11` on `codex/517-kill-switch-phase1`.

## DeepSeek

- Job: `job_404c5720-e40d-4f50-8593-0a40bbe8c4e8`
- Session: `3a074f4a-d21e-4302-89bc-8b3897f9c151`
- Source: sent through direct API auth after approval preflight.
- Verdict: approved.
- Blocking findings: none.
- Non-blocking implementation refinements:
  - Add an integration-style startup fail-closed test across config, durable store, and state recovery.
  - Keep `[risk.kill_switch]` optional so existing configs continue to parse; validate sub-fields only when enabled/present.
  - Cover all illegal direct re-arm transitions, not only `Flat -> Armed`.

## GLM

- Job: `job_b3ce51f3-c017-483e-a6d3-0595ac094fde`
- Session: `20260601230832701a33a8f0c14905`
- Source: sent through direct API auth after approval preflight.
- Verdict: approved.
- Blocking findings: none.
- Non-blocking implementation refinements:
  - Add explicit tests for both `Flat -> Armed` and `Halted -> Armed` reset gates.
  - Include an evidence schema version from the first store implementation.
  - Defer concurrency tests to Phase 2 admission-latch work, but keep the Phase 1 API compatible with later shared latch usage.

## Gate Result

The Phase 1 plan has unanimous approval from the two usable external reviewers for this session. Phase 1 implementation may start under TDD. Before Phase 2 begins, the Phase 1 exact diff must receive another external review from all usable reviewers and must have unanimous approval.
