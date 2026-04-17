# Session State

- Current phase: draft PR created, verification complete, awaiting user-gated external adversarial review
- Current branch: `issue-109-resolution-basis-generalization`
- Local checkpoint: current `HEAD` of `issue-109-resolution-basis-generalization`
- Draft PR: `https://github.com/seungpyoson/bolt-v2/pull/191`
- Pending `EXTERNAL-INPUT` IDs: none
- Next action: if the user approves, send one or both prompts from `planning/109/EXTERNAL-REVIEW-PROMPTS.md` to an external reviewer family and process any resulting findings to `FIXED` or `DISPROVEN`
- Resume verification commands:
  - `cargo fmt --check`
  - `cargo test --test polymarket_catalog --test ruleset_selector --test platform_runtime`
  - `cargo test phase1_runtime_resolution_basis_requires_matching_reference_venue_family`
  - `cargo test phase1_runtime_rejects_invalid_resolution_basis_format`
  - `cargo test phase1_runtime_eth_chainlink_basis_requires_matching_reference_venue_family`
