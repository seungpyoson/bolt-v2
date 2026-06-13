# Adversarial Review Prompt: Global Shadow Execution Policy

Review the plan/spec packet for `specs/621-global-shadow-execution-policy/`.

Focus on finding blockers before implementation. Treat any real issue as a finding.

Required review questions:

1. Does the architecture truly remove PR #621's strategy-local `submit_orders` ownership, or does it leave hidden strategy-bound execution policy?
2. Does root `[runtime]` ownership satisfy group-by-change, or should the mode live somewhere else?
3. Does the proposed `src/bolt_v3_order_execution.rs` boundary stay separate from `src/bolt_v3_order_intent.rs`, `src/bolt_v3_submit_admission.rs`, and strategy economics?
4. Can shadow mode still mutate venue state through NT-managed paths not covered by the plan?
5. Does the shared submit helper preserve PR #621 evidence ordering and shadow PnL semantics?
6. Does the plan introduce a dual submit path, venue capability matrix, hardcoded runtime policy, or NT lifecycle reimplementation?
7. Are the TDD and verification tasks sufficient to prove the invariant at current repo standards?

Return one of:

- `VERDICT: APPROVE` with any non-blocking notes
- `VERDICT: BLOCK` with findings ordered by severity

Review only the plan/spec packet. Do not propose or apply code changes.
