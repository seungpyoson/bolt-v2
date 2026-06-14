# Adversarial Review Prompt: Global Shadow Execution Policy

Review the plan/spec packet for `specs/621-global-shadow-execution-policy/`.

Focus on finding blockers before implementation. Treat any real issue as a finding.

Important scope boundary:

- This PR guarantees global shadow execution for the current shipped, source-integrity-locked `binary_oracle_edge_taker` strategy.
- Source fences are CI guardrails for known/common direct mutation and policy-fabrication bypass forms.
- Source fences are not claimed to be a complete firewall over every public NautilusTrader transport API available to arbitrary future strategy code.
- Compile-time isolation for future strategies is out of scope for this PR and tracked in GitHub issue #710.

Required review questions:

1. Does the architecture truly remove PR #621's strategy-local `submit_orders` ownership, or does it leave hidden strategy-bound execution policy?
2. Does root `[runtime]` ownership satisfy group-by-change, or should the mode live somewhere else?
3. Does the proposed `src/bolt_v3_order_execution.rs` boundary stay separate from `src/bolt_v3_order_intent.rs`, `src/bolt_v3_submit_admission.rs`, and strategy economics?
4. Can the current shipped production strategy still mutate venue state in shadow mode through strategy-originated NT mutation APIs or NT-managed paths not covered by the plan?
5. Does the shared submit helper preserve PR #621 evidence ordering and shadow PnL semantics?
6. Does the plan introduce a dual submit path, venue capability matrix, hardcoded runtime policy, speculative unused NT wrappers, or NT lifecycle reimplementation?
7. Is the pinned NT `StrategyConfig` managed-action audit complete enough to justify the reject list?
8. Are the TDD, source-fence guardrails, source-integrity review controls, and verification tasks sufficient to prove the scoped invariant at current repo standards?

Return one of:

- `VERDICT: APPROVE` with any non-blocking notes
- `VERDICT: BLOCK` with findings ordered by severity

Review only the plan/spec packet. Do not propose or apply code changes.
