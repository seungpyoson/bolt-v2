# Implementation Plan: Production Trade Readiness

**Branch**: `goal/024-production-trade-readiness`
**PR**: #480
**Spec**: `specs/024-production-trade-readiness/spec.md`
**Tasks**: `specs/024-production-trade-readiness/tasks.md`

## Technical Context

**Language**: Rust
**Primary implementation files**: `src/bolt_v3_operator_artifacts.rs`, `src/bolt_v3_tiny_canary_evidence.rs`, `src/bolt_v3_live_canary_gate.rs`, `src/bolt_v3_live_node.rs`
**Primary tests**: `tests/bolt_v3_operator_artifacts.rs`, `tests/bolt_v3_tiny_canary_preconditions.rs`, `tests/bolt_v3_tiny_canary_operator.rs`, `tests/bolt_v3_live_canary_gate.rs`, `tests/bolt_v3_cli.rs`
**Verification**: focused Rust tests, `cargo fmt --check`, `git diff --check`, runtime-literal verifier, source/slop/hardcode/secret scans, GitHub CI, external model review.

## Evidence Baseline

The current investigation found:

- PR #480 is the active production trade-readiness consolidation PR on `goal/024-production-trade-readiness`.
- Historical PR #478 was closed by GitHub after the stale branch was renamed; it is superseded by #480 and must not be treated as active readiness scope.
- PR #479 is separate #466 verifier decomposition work and is out of scope.
- #369 and #385 are open.
- #409 is open, but current source already contains PortfolioSnapshot capture; the task list must verify whether this is ready to close or still needs issue evidence.
- #360 is closed and remains historical tiny-canary readiness context, not production-readiness completion.
- T038 no-submit evidence is historically satisfied only for no-submit; final-packet T131/T122 remains unproven.
- The old `t038-operator-config-snapshot` branch has unique commits, but current source contains later no-submit/SBE work and recorded EC2/EIP no-submit proof. It must not be ported wholesale.
- The active readiness branch currently has source collectors for release manifest, host clock, market window, single-runner lock, and cancel-if-open.
- The active readiness branch now exposes collector functions for venue account/open orders/positions, funding/margin, egress identity, CLOB V2 signing/collateral/fee behavior, NT accepted/venue pending, partial fill, and network partition. It still does not expose collector functions for panic/service policy.

See `specs/024-production-trade-readiness/evidence.md` for commands and exact outputs summarized.

## Constraints

- One readiness PR.
- No order-intent-layer work.
- No #466 decomposition-ledger work.
- No hardcoded runtime values.
- SSM remains the only secret source.
- No secret display.
- No live/no-submit/trading operations until the prerequisite artifacts and verification chain are ready. The operator has approved the listed operations, but approval does not bypass prerequisites.

## Strategy

1. Finish task-list approval first.
2. Remove PR #480 scope contamination before deeper implementation.
3. Implement missing source-owned evidence collectors in TDD slices.
4. Produce real current-head runtime artifacts.
5. Assemble and verify final packet.
6. Run final exact-head verification and external review.
7. Run approved final-packet no-submit, then tiny-capital canary.
